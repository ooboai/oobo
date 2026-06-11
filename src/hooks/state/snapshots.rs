//! Git blob snapshot functions for file-level attribution tracking.

use std::process::{Command, Stdio};

use crate::error::Result;
use crate::hooks::store;

use super::FileEditPair;

/// Resolve the worktree root for the current directory.
/// Returns canonicalized path to avoid macOS `/var` vs `/private/var` mismatches.
pub(super) fn resolve_worktree(project_root: &str) -> Option<String> {
    if project_root.is_empty() {
        return None;
    }
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = Command::new(git)
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .output()
        .ok()?;

    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout)
            .replace('\r', "")
            .trim()
            .to_string();
        let canonical = std::fs::canonicalize(&raw)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(raw);
        Some(canonical)
    } else {
        None
    }
}

/// Capture the pre-agent file state: snapshot currently dirty files in the
/// worktree. Called on `before-submit-prompt`  --  any worktree changes at this
/// moment are human edits (the agent hasn't started yet).
pub fn snapshot_pre_agent_state(project_root: &str, session_id: &str) -> Result<()> {
    let snapshots = snapshot_dirty_files(project_root);
    if snapshots.is_none() {
        return Ok(());
    }
    super::mutate(project_root, session_id, |state| {
        state.pre_agent_snapshots = snapshots;
    })
}

fn snapshot_dirty_files(project_root: &str) -> Option<std::collections::HashMap<String, String>> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut dirty_files: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for args in [
        &["diff", "--name-only", "HEAD"][..],
        &["ls-files", "--others", "--exclude-standard"][..],
    ] {
        if let Ok(o) = Command::new(&git)
            .args(args)
            .current_dir(project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    if !line.is_empty() && seen.insert(line.to_string()) {
                        dirty_files.push(line.to_string());
                    }
                }
            }
        }
    }

    if dirty_files.is_empty() {
        // No dirty files → HEAD IS the pre-agent state. No snapshot needed;
        // at commit time we'll use HEAD~1 as the baseline.
        return None;
    }

    let mut snapshots = std::collections::HashMap::new();
    for file in &dirty_files {
        if let Some(hash) = hash_object(&git, project_root, file) {
            snapshots.insert(file.clone(), hash);
        }
    }
    if snapshots.is_empty() {
        None
    } else {
        Some(snapshots)
    }
}

pub(super) fn hash_object(git: &str, project_root: &str, file: &str) -> Option<String> {
    let abs_path = std::path::Path::new(project_root).join(file);
    if !abs_path.exists() {
        return None;
    }
    let output = Command::new(git)
        .args(["hash-object", "-w"])
        .arg(&abs_path)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8_lossy(&output.stdout)
        .replace('\r', "")
        .trim()
        .to_string();
    if hash.is_empty() {
        None
    } else {
        Some(hash)
    }
}

/// Snapshot files edited by this session into git's object store.
/// For each file, runs `git hash-object -w <file>` and stores the blob hash
/// in the session's `file_snapshots` map.
pub fn snapshot_session_files(
    project_root: &str,
    session_id: &str,
    files: &[String],
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut new_snapshots = std::collections::HashMap::new();
    for file in files {
        if let Some(hash) = hash_object(&git, project_root, file) {
            new_snapshots.insert(file.clone(), hash);
        }
    }
    if new_snapshots.is_empty() {
        return Ok(());
    }
    super::mutate(project_root, session_id, |state| {
        let mut snapshots = state.file_snapshots.take().unwrap_or_default();
        snapshots.extend(new_snapshots);
        state.file_snapshots = Some(snapshots);
    })
}

/// Capture the git blob hash of a file BEFORE an AI edit (preToolUse).
/// Stored in `pre_edit_pending` until paired by `record_post_edit_file`.
pub fn snapshot_pre_edit_file(project_root: &str, session_id: &str, rel_path: &str) -> Result<()> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let hash = match hash_object(&git, project_root, rel_path) {
        Some(h) => h,
        None => {
            // File doesn't exist yet  --  record a sentinel so we know it was created.
            "0000000000000000000000000000000000000000".to_string()
        }
    };
    super::mutate(project_root, session_id, |state| {
        let mut pending = state.pre_edit_pending.take().unwrap_or_default();
        pending.insert(rel_path.to_string(), hash);
        state.pre_edit_pending = Some(pending);
    })
}

/// Pair a pre-edit blob with the post-edit blob (postToolUse). If a pre-edit
/// snapshot exists for this file, creates a `FileEditPair` and appends it
/// to the edit chain. Also records the post-edit blob in `file_snapshots`.
pub fn record_post_edit_file(
    project_root: &str,
    session_id: &str,
    rel_path: &str,
    tool_name: Option<&str>,
) -> Result<()> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let post_blob = match hash_object(&git, project_root, rel_path) {
        Some(h) => h,
        None => return Ok(()),
    };
    super::mutate(project_root, session_id, |state| {
        let mut snapshots = state.file_snapshots.take().unwrap_or_default();
        snapshots.insert(rel_path.to_string(), post_blob.clone());
        state.file_snapshots = Some(snapshots);

        let pre_blob = state
            .pre_edit_pending
            .as_mut()
            .and_then(|p| p.remove(rel_path));

        if let Some(pre) = pre_blob {
            if pre != post_blob {
                state.event_seq += 1;
                let now = chrono::Utc::now();
                let pair = FileEditPair {
                    pre_blob: pre,
                    post_blob,
                    tool_name: tool_name.map(std::string::ToString::to_string),
                    timestamp: now.timestamp(),
                    seq: state.event_seq,
                    timestamp_us: now.timestamp_micros(),
                };
                let mut chain = state.file_edit_chain.take().unwrap_or_default();
                chain.entry(rel_path.to_string()).or_default().push(pair);
                state.file_edit_chain = Some(chain);
            }
        }
    })
}

// ── Foreign-repo capture ───────────────────────────────────────────────
//
// When a session edits a file in a repository other than its origin, the
// blob snapshots are written into *that repo's* object database and the
// capture state is tracked in the session's per-repo bucket
// ([`super::RepoCapture`]). This keeps the foreign repo self-contained:
// its attribution data never depends on the origin repo being present.

/// Capture the pre-edit blob of a file in a foreign repo (preToolUse).
pub fn snapshot_pre_edit_file_in_repo(
    origin_root: &str,
    session_id: &str,
    repo_root: &str,
    rel_path: &str,
) -> Result<()> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let hash = hash_object(&git, repo_root, rel_path)
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());
    let repo_root = repo_root.to_string();
    super::mutate(origin_root, session_id, move |state| {
        let capture = state.foreign_capture_mut(&repo_root);
        capture
            .pre_edit_pending
            .get_or_insert_with(Default::default)
            .insert(rel_path.to_string(), hash);
    })
}

/// Pair pre/post edit blobs for a file in a foreign repo (postToolUse).
pub fn record_post_edit_file_in_repo(
    origin_root: &str,
    session_id: &str,
    repo_root: &str,
    rel_path: &str,
    tool_name: Option<&str>,
) -> Result<()> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let Some(post_blob) = hash_object(&git, repo_root, rel_path) else {
        return Ok(());
    };
    let repo_root = repo_root.to_string();
    super::mutate(origin_root, session_id, move |state| {
        let pre_blob = {
            let capture = state.foreign_capture_mut(&repo_root);
            capture
                .file_snapshots
                .get_or_insert_with(Default::default)
                .insert(rel_path.to_string(), post_blob.clone());
            capture
                .pre_edit_pending
                .as_mut()
                .and_then(|p| p.remove(rel_path))
        };

        if let Some(pre) = pre_blob {
            if pre != post_blob {
                state.event_seq += 1;
                let seq = state.event_seq;
                let now = chrono::Utc::now();
                state
                    .foreign_capture_mut(&repo_root)
                    .file_edit_chain
                    .get_or_insert_with(Default::default)
                    .entry(rel_path.to_string())
                    .or_default()
                    .push(FileEditPair {
                        pre_blob: pre,
                        post_blob,
                        tool_name: tool_name.map(std::string::ToString::to_string),
                        timestamp: now.timestamp(),
                        seq,
                        timestamp_us: now.timestamp_micros(),
                    });
            }
        }
    })
}

/// Snapshot a set of files in a foreign repo (e.g. subagent-reported
/// modified_files) and mark them as touched this turn.
pub fn snapshot_files_in_repo(
    origin_root: &str,
    session_id: &str,
    repo_root: &str,
    files: &[String],
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut new_snapshots = std::collections::HashMap::new();
    for file in files {
        if let Some(hash) = hash_object(&git, repo_root, file) {
            new_snapshots.insert(file.clone(), hash);
        }
    }
    let repo_root = repo_root.to_string();
    let files: Vec<String> = files.to_vec();
    super::mutate(origin_root, session_id, move |state| {
        let now = chrono::Utc::now().timestamp();
        let capture = state.foreign_capture_mut(&repo_root);
        if capture.turn_started_at.is_none() {
            capture.turn_started_at = Some(now);
        }
        for file in &files {
            capture
                .edited_files
                .get_or_insert_with(Default::default)
                .insert(file.clone());
            capture
                .turn_files
                .get_or_insert_with(Default::default)
                .insert(file.clone());
        }
        if !new_snapshots.is_empty() {
            capture
                .file_snapshots
                .get_or_insert_with(Default::default)
                .extend(new_snapshots);
        }
    })
}

/// Clean up stale session state older than `max_age_secs`.
/// Ended sessions (those with `ended_at` set) use a shorter grace period
/// of 1 hour  --  long enough for the user to commit after exiting the agent.
pub fn cleanup_stale(project_root: &str, max_age_secs: i64) {
    const ENDED_GRACE_SECS: i64 = 3600;
    let now = chrono::Utc::now().timestamp();
    let sessions = store::list_for_project(project_root);
    for s in sessions {
        let stale = if let Some(ended) = s.ended_at {
            now - ended > ENDED_GRACE_SECS
        } else {
            now - s.updated_at > max_age_secs
        };
        if stale {
            store::remove(project_root, &s.session_id);
        }
    }
    store::cleanup_buffer(max_age_secs);
}
