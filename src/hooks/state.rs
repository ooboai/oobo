use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Active session state — written to `<git-common-dir>/oobo-sessions/<session_id>.json`.
///
/// Lightweight and ephemeral: tracks which agent sessions are active right now.
/// Used by the post-commit hook to link sessions to commits.
///
/// The `worktree` field enables correct session→commit linking when multiple
/// agents work in parallel via git worktrees (all worktrees share one `.git`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSession {
    pub session_id: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
}

/// Resolve the shared git directory that all worktrees use.
/// For main worktree: returns `<repo>/.git`
/// For linked worktree: returns `<main-repo>/.git` (the common dir)
/// Falls back to `<project_root>/.git` if git isn't available.
fn git_common_dir(project_root: &str) -> PathBuf {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = Command::new(git)
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .output();

    if let Ok(o) = output {
        if o.status.success() {
            let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let p = Path::new(&raw);
            if p.is_absolute() {
                return p.to_path_buf();
            }
            return Path::new(project_root).join(p);
        }
    }

    Path::new(project_root).join(".git")
}

fn sessions_dir(project_root: &str) -> PathBuf {
    git_common_dir(project_root).join("oobo-sessions")
}

fn session_path(project_root: &str, session_id: &str) -> PathBuf {
    let sanitized = sanitize_session_id(session_id);
    sessions_dir(project_root).join(format!("{sanitized}.json"))
}

/// Reject anything that isn't a plausible session ID (UUID-like: alphanumeric + hyphens).
fn sanitize_session_id(id: &str) -> &str {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        id
    } else {
        "invalid"
    }
}

/// Resolve the worktree root for the current directory.
/// Returns canonicalized path to avoid macOS `/var` vs `/private/var` mismatches.
fn resolve_worktree(project_root: &str) -> Option<String> {
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
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let canonical = std::fs::canonicalize(&raw)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(raw);
        Some(canonical)
    } else {
        None
    }
}

/// Write a new active session state file.
pub fn write_session(
    project_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
) -> Result<()> {
    let dir = sessions_dir(project_root);
    fs::create_dir_all(&dir)?;

    let now = chrono::Utc::now().timestamp();
    let worktree = resolve_worktree(project_root);
    let state = ActiveSession {
        session_id: session_id.to_string(),
        agent: agent.to_string(),
        model: model.map(|s| s.to_string()),
        worktree,
        transcript_path: None,
        started_at: now,
        updated_at: now,
    };

    let json = serde_json::to_string_pretty(&state)?;
    let path = session_path(project_root, session_id);
    fs::write(&path, json)?;

    Ok(())
}

/// Update the timestamp on an existing session (turn ended, session continues).
/// Also stores `transcript_path` if provided by the hook event.
pub fn touch_session(
    project_root: &str,
    session_id: &str,
    transcript_path: Option<&str>,
) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    state.updated_at = chrono::Utc::now().timestamp();
    if let Some(tp) = transcript_path {
        if !tp.is_empty() {
            state.transcript_path = Some(tp.to_string());
        }
    }

    let json = serde_json::to_string_pretty(&state)?;
    fs::write(&path, json)?;

    Ok(())
}

/// Remove a session state file (session ended).
pub fn remove_session(project_root: &str, session_id: &str) {
    let path = session_path(project_root, session_id);
    let _ = fs::remove_file(path);
}

/// List all currently active sessions for a project.
pub fn active_sessions(project_root: &str) -> Vec<ActiveSession> {
    read_all_sessions(project_root)
}

/// Max session age before auto-cleanup (6 hours).
const STALE_SESSION_SECS: i64 = 6 * 3600;

/// List active sessions filtered to only those belonging to the given worktree.
/// Sessions without a worktree field (pre-upgrade) are included in all worktrees
/// for backward compatibility. Stale sessions (>6h) are evicted automatically.
pub fn active_sessions_for_worktree(project_root: &str) -> Vec<ActiveSession> {
    cleanup_stale(project_root, STALE_SESSION_SECS);
    let current_wt = resolve_worktree(project_root);
    let all = read_all_sessions(project_root);

    match current_wt {
        Some(wt) => all
            .into_iter()
            .filter(|s| match &s.worktree {
                Some(session_wt) => {
                    let canonical = std::fs::canonicalize(session_wt)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| session_wt.clone());
                    canonical == wt
                }
                None => true,
            })
            .collect(),
        None => all,
    }
}

fn read_all_sessions(project_root: &str) -> Vec<ActiveSession> {
    let dir = sessions_dir(project_root);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(state) = serde_json::from_str::<ActiveSession>(&content) {
                    sessions.push(state);
                }
            }
        }
    }

    sessions
}

/// Clean up stale session files (older than the given threshold in seconds).
pub fn cleanup_stale(project_root: &str, max_age_secs: i64) {
    let now = chrono::Utc::now().timestamp();
    let dir = sessions_dir(project_root);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn init_git_repo(root: &Path) {
        std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
    }

    #[test]
    fn test_session_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);

        let root_str = root.to_str().unwrap();
        write_session(root_str, "sess-1", "cursor", Some("claude-opus-4")).unwrap();
        let sessions = active_sessions(root_str);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "sess-1");
        assert_eq!(sessions[0].agent, "cursor");
        assert_eq!(sessions[0].model.as_deref(), Some("claude-opus-4"));
        assert!(sessions[0].worktree.is_some());

        touch_session(root_str, "sess-1", None).unwrap();
        let sessions = active_sessions(root_str);
        assert!(sessions[0].updated_at >= sessions[0].started_at);

        remove_session(root_str, "sess-1");
        let sessions = active_sessions(root_str);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_worktree_filtering() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);

        let root_str = root.to_str().unwrap();
        let sessions_dir = git_common_dir(root_str).join("oobo-sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        let this_wt = resolve_worktree(root_str).unwrap();
        let now = chrono::Utc::now().timestamp();

        let matching = ActiveSession {
            session_id: "s1".into(),
            agent: "cursor".into(),
            model: None,
            worktree: Some(this_wt.clone()),
            transcript_path: None,
            started_at: now,
            updated_at: now,
        };
        let other_wt = ActiveSession {
            session_id: "s2".into(),
            agent: "claude".into(),
            model: None,
            worktree: Some("/other/worktree".into()),
            transcript_path: None,
            started_at: now,
            updated_at: now,
        };
        let no_wt = ActiveSession {
            session_id: "s3".into(),
            agent: "gemini".into(),
            model: None,
            worktree: None,
            transcript_path: None,
            started_at: now,
            updated_at: now,
        };

        for s in [&matching, &other_wt, &no_wt] {
            let json = serde_json::to_string_pretty(s).unwrap();
            fs::write(sessions_dir.join(format!("{}.json", s.session_id)), json).unwrap();
        }

        let all = active_sessions(root_str);
        assert_eq!(all.len(), 3);

        let filtered = active_sessions_for_worktree(root_str);
        assert_eq!(filtered.len(), 2);
        let ids: Vec<&str> = filtered.iter().map(|s| s.session_id.as_str()).collect();
        assert!(ids.contains(&"s1"));
        assert!(ids.contains(&"s3"));
        assert!(!ids.contains(&"s2"));
    }

    #[test]
    fn test_sanitize_session_id_allows_valid() {
        assert_eq!(
            sanitize_session_id("normal-session-id"),
            "normal-session-id"
        );
        assert_eq!(sanitize_session_id("abc-123-def"), "abc-123-def");
        assert_eq!(
            sanitize_session_id("2c97dced-3950-482e-b101-9eb7d1b18cf5"),
            "2c97dced-3950-482e-b101-9eb7d1b18cf5"
        );
    }

    #[test]
    fn test_sanitize_session_id_blocks_traversal() {
        assert_eq!(sanitize_session_id("../../../etc/passwd"), "invalid");
        assert_eq!(sanitize_session_id("../../secret"), "invalid");
        assert_eq!(sanitize_session_id("foo/bar"), "invalid");
        assert_eq!(sanitize_session_id("../"), "invalid");
        assert_eq!(sanitize_session_id(".."), "invalid");
        assert_eq!(sanitize_session_id(""), "invalid");
    }
}
