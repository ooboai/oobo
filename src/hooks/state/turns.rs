//! Turn finalization  --  snapshot current turn state into git-backed storage.

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
    // Link to foreign repos this turn also edited (finished separately by
    // `finish_foreign_turns`).
    let mut cross: Vec<String> = state
        .foreign_repos
        .as_ref()
        .map(|repos| {
            repos
                .iter()
                .filter(|(_, c)| c.turn_files.as_ref().is_some_and(|f| !f.is_empty()))
                .map(|(root, _)| root.clone())
                .collect()
        })
        .unwrap_or_default();
    cross.sort();
    snapshot.cross_repo = cross;

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
    // Reset per-turn edit chain  --  each turn gets a fresh chain.
    state.pre_edit_pending = None;
    state.file_edit_chain = None;
    state.bump();
    store::write(project_root, session_id, &state)?;

    Ok(Some(snapshot_id))
}

/// Finish the current turn for every *foreign* repo this session touched,
/// writing a provenance-only turn snapshot into each foreign repo's own
/// git ref store. Returns `(repo_root, snapshot_id)` pairs.
///
/// The snapshot carries the files + pre/post blobs (full attribution data)
/// but no transcript/tool-call memory  --  the conversation lives with the
/// session's origin. Each foreign repo keeps an independent turn index and
/// parent chain, counting only the turns that touched it.
pub fn finish_foreign_turns(
    origin_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let Some(mut state) = store::read(origin_root, session_id) else {
        return Ok(Vec::new());
    };
    let Some(foreign) = state.foreign_repos.as_mut() else {
        return Ok(Vec::new());
    };

    let source = crate::core::tool::normalize_source(agent);
    let model = model.map(str::to_string).or_else(|| state.model.clone());
    let session_id_owned = state.session_id.clone();
    let mut written = Vec::new();

    for (repo_root, capture) in foreign.iter_mut() {
        let turn_files: Vec<String> = capture
            .turn_files
            .as_ref()
            .map(|f| f.iter().cloned().collect())
            .unwrap_or_default();
        if turn_files.is_empty() {
            continue;
        }

        let worktree_id = crate::git::turns::worktree_id(repo_root);
        let project_id = crate::project::id_for_root(repo_root);
        let mut snapshot = TurnSnapshot::new(
            &project_id,
            &worktree_id,
            source,
            &session_id_owned,
            capture.turn_index,
        );
        snapshot
            .parent_id
            .clone_from(&capture.last_turn_snapshot_id);
        snapshot.started_at = capture.turn_started_at;
        snapshot.ended_at = Some(chrono::Utc::now().timestamp());
        snapshot.model.clone_from(&model);
        // Point back at the session's origin, where the conversation lives.
        if !origin_root.is_empty() {
            snapshot.cross_repo = vec![origin_root.to_string()];
        }

        let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
        let mut files = turn_files;
        files.sort();
        snapshot.files = files
            .into_iter()
            .map(|path| {
                let (pre, post, gap) = if let Some(pairs) = capture
                    .file_edit_chain
                    .as_ref()
                    .and_then(|c| c.get(&path))
                    .filter(|pairs| !pairs.is_empty())
                {
                    let current = super::snapshots::hash_object(&git, repo_root, &path);
                    (
                        Some(pairs.first().unwrap().pre_blob.clone()),
                        Some(pairs.last().unwrap().post_blob.clone()),
                        chain_has_gap(pairs, current.as_deref()),
                    )
                } else {
                    (
                        None,
                        capture
                            .file_snapshots
                            .as_ref()
                            .and_then(|s| s.get(&path))
                            .cloned(),
                        false,
                    )
                };
                TurnFileSnapshot {
                    pre_blob: pre,
                    post_blob: post,
                    capture_gap: gap,
                    path,
                }
            })
            .collect();

        let snapshot_id = snapshot.id.clone();
        if crate::git::turns::write_turn_snapshot(repo_root, snapshot).is_err() {
            continue;
        }

        capture.last_turn_snapshot_id = Some(snapshot_id.clone());
        capture.turn_index += 1;
        capture.turn_started_at = None;
        capture.turn_files = None;
        capture.pre_edit_pending = None;
        capture.file_edit_chain = None;
        written.push((repo_root.clone(), snapshot_id));
    }

    if !written.is_empty() {
        state.bump();
        store::write(origin_root, session_id, &state)?;
    }

    Ok(written)
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
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    files
        .into_iter()
        .map(|path| {
            let (pre, post, gap) = if let Some(chain) = state
                .file_edit_chain
                .as_ref()
                .and_then(|c| c.get(&path))
                .filter(|pairs| !pairs.is_empty())
            {
                let pre = Some(chain.first().unwrap().pre_blob.clone());
                let post = Some(chain.last().unwrap().post_blob.clone());
                let current = super::snapshots::hash_object(&git, project_root, &path);
                (pre, post, chain_has_gap(chain, current.as_deref()))
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
                (pre, post, false)
            };
            TurnFileSnapshot {
                pre_blob: pre,
                post_blob: post,
                capture_gap: gap,
                path,
            }
        })
        .collect()
}

/// True when the captured edit chain doesn't fully explain the file's
/// current content: an interior discontinuity means another writer slipped
/// in between captured edits; a terminal mismatch means the file drifted
/// after the last captured edit. `current` is the file's on-disk blob hash
/// at turn end (`None` = file unreadable/deleted, which we don't treat as
/// a gap on its own).
fn chain_has_gap(chain: &[super::FileEditPair], current: Option<&str>) -> bool {
    if chain.windows(2).any(|w| w[0].post_blob != w[1].pre_blob) {
        return true;
    }
    match (chain.last(), current) {
        (Some(last), Some(cur)) => last.post_blob != cur,
        _ => false,
    }
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
