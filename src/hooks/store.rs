//! Persistent storage for hook-session state.
//!
//! Three backends, tried in order:
//!
//! 1. **SQLite `hook_sessions` table** — primary. One row per
//!    `(project_id, session_id)`. Used whenever a project can be
//!    resolved from `project_root`.
//! 2. **Buffer files** — `~/.oobo/tmp/hook-buffer/<sid>.json`. Fallback
//!    when no project can be resolved (typical case: Cursor starts a
//!    session before `git init` has been run, or the DB is transiently
//!    unavailable).
//! 3. **Legacy `.git/oobo-sessions/<sid>.json`** — read-only. Files
//!    written by oobo 0.1.x. Lazily imported into the DB on first hit
//!    and then deleted.
//!
//! Write path: always try DB first, fall through to the buffer on any
//! failure (including "no project root"). On a successful DB write, any
//! lingering buffer or legacy file for the same session is cleaned up.
//!
//! Read path: DB → buffer → legacy (imports legacy on hit).

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
    if !project_root.is_empty() {
        if let Some(state) = read_from_db(project_root, session_id) {
            return Some(state);
        }
    }
    if let Some(state) = read_from_buffer(session_id) {
        return Some(state);
    }
    if !project_root.is_empty() {
        if let Some(state) = read_from_legacy(project_root, session_id) {
            // Lazy-import: persist to DB and drop the legacy file.
            if write_to_db(project_root, session_id, &state).is_ok() {
                let _ = fs::remove_file(legacy_path(project_root, session_id));
            }
            return Some(state);
        }
    }
    None
}

/// Write a session's state. Prefers the DB when a project can be
/// resolved; otherwise writes to the pre-git-init buffer file.
///
/// On a successful DB write, any buffer or legacy file for the same
/// session is removed to avoid drift.
pub fn write(
    project_root: &str,
    session_id: &str,
    state: &ActiveSession,
) -> std::io::Result<()> {
    if !project_root.is_empty() {
        if write_to_db(project_root, session_id, state).is_ok() {
            let _ = fs::remove_file(buffer_path(session_id));
            let _ = fs::remove_file(legacy_path(project_root, session_id));
            return Ok(());
        }
    }
    write_to_buffer(session_id, state)
}

/// True if a session's state exists in any backend.
pub fn exists(project_root: &str, session_id: &str) -> bool {
    if !project_root.is_empty() && db_exists(project_root, session_id) {
        return true;
    }
    if buffer_path(session_id).exists() {
        return true;
    }
    !project_root.is_empty() && legacy_path(project_root, session_id).exists()
}

/// Remove a session's state from every backend.
pub fn remove(project_root: &str, session_id: &str) {
    if !project_root.is_empty() {
        let _ = delete_from_db(project_root, session_id);
        let _ = fs::remove_file(legacy_path(project_root, session_id));
    }
    let _ = fs::remove_file(buffer_path(session_id));
}

/// All active sessions associated with `project_root`. Merges DB rows
/// with any remaining legacy files for the same project. The buffer
/// directory is scanned too — any session whose payload records a
/// matching worktree is included.
pub fn list_for_project(project_root: &str) -> Vec<ActiveSession> {
    let mut out: Vec<ActiveSession> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // DB rows for this project.
    if !project_root.is_empty() {
        if let Ok(db) = crate::db::Db::open() {
            let pid = match crate::project::ensure_stable(&db, project_root) {
                Ok(id) => id,
                Err(_) => crate::project::id_for_root(project_root),
            };
            if let Ok(mut stmt) = db
                .conn
                .prepare("SELECT session_id, payload FROM hook_sessions WHERE project_id = ?1")
            {
                if let Ok(rows) = stmt.query_map([&pid], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                }) {
                    for r in rows.flatten() {
                        if let Ok(state) = serde_json::from_str::<ActiveSession>(&r.1) {
                            if seen.insert(state.session_id.clone()) {
                                out.push(state);
                            }
                        }
                    }
                }
            }
        }
    }

    // Legacy files for this project (not yet imported).
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

    // Buffer files — include any whose worktree matches.
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
                        .map(|wt| worktree_matches(wt, project_root))
                        .unwrap_or(false);
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

// ── DB backend ─────────────────────────────────────────────────────────

fn with_db<T>(f: impl FnOnce(&crate::db::Db) -> T) -> Option<T> {
    crate::db::Db::open().ok().map(|db| f(&db))
}

fn resolve_project_id(db: &crate::db::Db, project_root: &str) -> Option<String> {
    if project_root.is_empty() {
        return None;
    }
    crate::project::ensure_stable(db, project_root).ok()
}

fn read_from_db(project_root: &str, session_id: &str) -> Option<ActiveSession> {
    with_db(|db| {
        let pid = resolve_project_id(db, project_root)?;
        let payload: String = db
            .conn
            .query_row(
                "SELECT payload FROM hook_sessions \
                 WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![&pid, session_id],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str::<ActiveSession>(&payload).ok()
    })
    .flatten()
}

fn db_exists(project_root: &str, session_id: &str) -> bool {
    with_db(|db| {
        let Some(pid) = resolve_project_id(db, project_root) else {
            return false;
        };
        db.conn
            .query_row(
                "SELECT 1 FROM hook_sessions \
                 WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![&pid, session_id],
                |row| row.get::<_, i64>(0),
            )
            .is_ok()
    })
    .unwrap_or(false)
}

fn write_to_db(
    project_root: &str,
    session_id: &str,
    state: &ActiveSession,
) -> Result<(), String> {
    let db = crate::db::Db::open().map_err(|e| format!("open db: {e}"))?;
    let pid =
        resolve_project_id(&db, project_root).ok_or_else(|| "no project_id".to_string())?;
    let payload = serde_json::to_string(state).map_err(|e| format!("serialize: {e}"))?;
    db.conn
        .execute(
            "INSERT INTO hook_sessions (project_id, session_id, payload, updated_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(project_id, session_id) DO UPDATE SET \
               payload = excluded.payload, updated_at = excluded.updated_at",
            rusqlite::params![&pid, session_id, &payload, state.updated_at],
        )
        .map_err(|e| format!("upsert hook_session: {e}"))?;
    Ok(())
}

fn delete_from_db(project_root: &str, session_id: &str) -> Result<(), String> {
    let db = crate::db::Db::open().map_err(|e| format!("open db: {e}"))?;
    let Some(pid) = resolve_project_id(&db, project_root) else {
        return Ok(());
    };
    db.conn
        .execute(
            "DELETE FROM hook_sessions WHERE project_id = ?1 AND session_id = ?2",
            rusqlite::params![&pid, session_id],
        )
        .map_err(|e| format!("delete hook_session: {e}"))?;
    Ok(())
}

// ── Buffer backend (pre-git-init + DB-failure fallback) ────────────────

fn read_from_buffer(session_id: &str) -> Option<ActiveSession> {
    let path = buffer_path(session_id);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<ActiveSession>(&content).ok()
}

fn write_to_buffer(session_id: &str, state: &ActiveSession) -> std::io::Result<()> {
    let dir = buffer_dir();
    fs::create_dir_all(&dir)?;
    let path = buffer_path(session_id);
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
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
        fs::canonicalize(p)
            .map(|b| b.to_string_lossy().to_string())
            .unwrap_or_else(|_| p.to_string())
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
            tool_usage: None,
            tool_failures: None,
            bash_commands: None,
            subagent_runs: None,
            thinking_duration_ms: None,
            compact_count: None,
            started_at: 1,
            updated_at: 1,
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
