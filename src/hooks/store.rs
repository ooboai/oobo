//! Persistent storage for hook-session state.
//!
//! Two backends, tried in order:
//!
//! 1. **Buffer files** — `~/.oobo/tmp/hook-buffer/<sid>.json`. Primary
//!    write target.
//! 2. **Legacy `.git/oobo-sessions/<sid>.json`** — read-only. Files
//!    written by oobo 0.1.x.
//!
//! Read path: buffer → legacy.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::state::ActiveSession;

// ── Paths ──────────────────────────────────────────────────────────────

fn buffer_dir() -> PathBuf {
    crate::paths::oobo_home().join("tmp").join("hook-buffer")
}

fn buffer_path(session_id: &str) -> PathBuf {
    let sanitized = sanitize(session_id);
    buffer_dir().join(format!("{sanitized}.json"))
}

fn legacy_dir(project_root: &str) -> PathBuf {
    crate::git::detect::resolve_git_common_dir(project_root).join("oobo-sessions")
}

fn legacy_path(project_root: &str, session_id: &str) -> PathBuf {
    let sanitized = sanitize(session_id);
    legacy_dir(project_root).join(format!("{sanitized}.json"))
}

fn sanitize(id: &str) -> &str {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        id
    } else {
        "invalid"
    }
}

// ── Public API ─────────────────────────────────────────────────────────

/// Read a session's state from whichever backend has it.
pub fn read(project_root: &str, session_id: &str) -> Option<ActiveSession> {
    if let Some(state) = read_from_buffer(session_id) {
        return Some(state);
    }
    if !project_root.is_empty() {
        if let Some(state) = read_from_legacy(project_root, session_id) {
            return Some(state);
        }
    }
    None
}

/// Write a session's state to the buffer file.
pub fn write(_project_root: &str, session_id: &str, state: &ActiveSession) -> std::io::Result<()> {
    write_to_buffer(session_id, state)
}

/// True if a session's state exists in any backend.
pub fn exists(project_root: &str, session_id: &str) -> bool {
    if buffer_path(session_id).exists() {
        return true;
    }
    !project_root.is_empty() && legacy_path(project_root, session_id).exists()
}

/// Remove a session's state from every backend.
pub fn remove(project_root: &str, session_id: &str) {
    if !project_root.is_empty() {
        let _ = fs::remove_file(legacy_path(project_root, session_id));
    }
    let _ = fs::remove_file(buffer_path(session_id));
}

/// All active sessions associated with `project_root`. Merges legacy
/// files with buffer files whose worktree matches.
pub fn list_for_project(project_root: &str) -> Vec<ActiveSession> {
    let mut out: Vec<ActiveSession> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Legacy files for this project.
    if !project_root.is_empty() {
        let dir = legacy_dir(project_root);
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(state) = serde_json::from_str::<ActiveSession>(&content) {
                        if seen.insert(state.session_id.clone()) {
                            out.push(state);
                        }
                    }
                }
            }
        }
    }

    // Buffer files.
    let dir = buffer_dir();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(state) = serde_json::from_str::<ActiveSession>(&content) {
                    let matches_root = state
                        .worktree
                        .as_deref()
                        .is_some_and(|wt| worktree_matches(wt, project_root));
                    if matches_root && seen.insert(state.session_id.clone()) {
                        out.push(state);
                    }
                }
            }
        }
    }

    out
}

/// Drop buffer entries older than `max_age_secs`. Called opportunistically.
pub fn cleanup_buffer(max_age_secs: i64) {
    let now = chrono::Utc::now().timestamp();
    let dir = buffer_dir();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(state) = serde_json::from_str::<ActiveSession>(&content) {
                    if now - state.updated_at > max_age_secs {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
    }
}

// ── Buffer backend ─────────────────────────────────────────────────────

fn read_from_buffer(session_id: &str) -> Option<ActiveSession> {
    let path = buffer_path(session_id);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<ActiveSession>(&content).ok()
}

fn write_to_buffer(session_id: &str, state: &ActiveSession) -> std::io::Result<()> {
    let dir = buffer_dir();
    fs::create_dir_all(&dir)?;
    let path = buffer_path(session_id);
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    atomic_write_json(&path, &json)
}

fn atomic_write_json(path: &Path, json: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(json.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

// ── Legacy backend (read-only) ─────────────────────────────────────────

fn read_from_legacy(project_root: &str, session_id: &str) -> Option<ActiveSession> {
    let path = legacy_path(project_root, session_id);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<ActiveSession>(&content).ok()
}

// ── Helpers ────────────────────────────────────────────────────────────

fn worktree_matches(worktree: &str, project_root: &str) -> bool {
    let canonical = |p: &str| {
        fs::canonicalize(p).map_or_else(|_| p.to_string(), |b| b.to_string_lossy().to_string())
    };
    canonical(worktree) == canonical(project_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_buffer_env() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("OOBO_HOME", tmp.path());
        // Force paths module to pick up the env var.
        tmp
    }

    fn mk_state(sid: &str) -> ActiveSession {
        ActiveSession {
            session_id: sid.to_string(),
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
            started_at: 1,
            updated_at: 1,
            ended_at: None,
        }
    }

    #[test]
    fn test_buffer_roundtrip_no_project_root() {
        let _env = fresh_buffer_env();
        let state = mk_state("buf-sid");
        // Empty project_root forces the buffer path.
        write("", "buf-sid", &state).unwrap();
        let back = read("", "buf-sid").expect("buffer hit");
        assert_eq!(back.session_id, "buf-sid");
        // Cleanup.
        remove("", "buf-sid");
        assert!(read("", "buf-sid").is_none());
    }

    #[test]
    fn test_sanitize_rejects_traversal() {
        assert_eq!(sanitize("../../etc/passwd"), "invalid");
        assert_eq!(sanitize("good-id-123"), "good-id-123");
        assert_eq!(sanitize(""), "invalid");
    }
}
