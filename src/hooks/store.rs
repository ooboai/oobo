//! Persistent storage for hook-session state.
//!
//! Two backends, tried in order:
//!
//! 1. **Buffer files**  --  `~/.oobo/tmp/hook-buffer/<sid>.json`. Primary
//!    write target.
//! 2. **Legacy `.git/oobo-sessions/<sid>.json`**  --  read-only. Files
//!    written by oobo 0.1.x.
//!
//! Read path: buffer → legacy.

use std::fs::{self, OpenOptions};
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

fn sanitize(id: &str) -> String {
    if id.is_empty() {
        return "invalid".to_string();
    }
    let safe: String = id
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '_',
        })
        .collect();
    let cleaned = safe.replace("..", "_");
    let trimmed = cleaned.trim_matches('.').to_string();
    if trimmed.is_empty() {
        return "invalid".to_string();
    }
    trimmed
}

// ── File locking ──────────────────────────────────────────────────────

/// Advisory lock on the session buffer file. Returns a guard that removes
/// the lock file on drop.
struct BufferLock {
    path: PathBuf,
}

impl BufferLock {
    fn acquire(session_id: &str) -> Option<Self> {
        let dir = buffer_dir();
        let _ = fs::create_dir_all(&dir);
        let lock_path = dir.join(format!("{}.lock", sanitize(session_id)));
        for _ in 0..50 {
            if OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
                .is_ok()
            {
                return Some(Self { path: lock_path });
            }
            if let Ok(meta) = fs::metadata(&lock_path) {
                if let Some(age) = meta.modified().ok().and_then(|m| m.elapsed().ok()) {
                    if age.as_secs() > 30 {
                        let _ = fs::remove_file(&lock_path);
                        continue;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}

impl Drop for BufferLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
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

/// Atomically read-modify-write a session's state, holding the buffer lock
/// across the entire cycle so concurrent processes cannot interleave.
pub fn read_modify_write<F>(project_root: &str, session_id: &str, f: F) -> std::io::Result<bool>
where
    F: FnOnce(&mut ActiveSession),
{
    let dir = buffer_dir();
    let _ = fs::create_dir_all(&dir);
    let _lock = BufferLock::acquire(session_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("could not acquire buffer lock for session {session_id}"),
        )
    })?;
    let Some(mut state) = read(project_root, session_id) else {
        return Ok(false);
    };
    f(&mut state);
    let path = buffer_path(session_id);
    let json = serde_json::to_string_pretty(&state).map_err(std::io::Error::other)?;
    atomic_write_json(&path, &json)?;
    Ok(true)
}

/// Like `read_modify_write` but the closure can return a value and the
/// write only happens when a state existed. Returns `None` when the
/// session doesn't exist.
pub fn read_modify_write_with<F, T>(
    project_root: &str,
    session_id: &str,
    f: F,
) -> std::io::Result<Option<T>>
where
    F: FnOnce(&mut ActiveSession) -> T,
{
    let dir = buffer_dir();
    let _ = fs::create_dir_all(&dir);
    let _lock = BufferLock::acquire(session_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("could not acquire buffer lock for session {session_id}"),
        )
    })?;
    let Some(mut state) = read(project_root, session_id) else {
        return Ok(None);
    };
    let result = f(&mut state);
    let path = buffer_path(session_id);
    let json = serde_json::to_string_pretty(&state).map_err(std::io::Error::other)?;
    atomic_write_json(&path, &json)?;
    Ok(Some(result))
}

/// Lock the session buffer, read the state, and return both the guard and
/// state for batched mutations. The caller MUST call `write_locked` to
/// persist changes while still holding the guard.
pub fn read_locked(project_root: &str, session_id: &str) -> Option<(BufferGuard, ActiveSession)> {
    let _ = fs::create_dir_all(buffer_dir());
    let lock = BufferLock::acquire(session_id)?;
    let state = read(project_root, session_id)?;
    Some((BufferGuard { _lock: lock }, state))
}

/// Write session state while holding the buffer guard (lock is already held).
pub fn write_locked(
    _guard: &BufferGuard,
    session_id: &str,
    state: &ActiveSession,
) -> std::io::Result<()> {
    let path = buffer_path(session_id);
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    atomic_write_json(&path, &json)
}

/// Opaque guard that holds the buffer lock. Dropping it releases the lock.
pub struct BufferGuard {
    _lock: BufferLock,
}

/// Atomically create a session if it doesn't exist yet, under the buffer
/// lock. Returns `true` if a new session was created, `false` if one
/// already existed (no-op). This eliminates the check-then-act race in
/// `write_session` where two concurrent `session-start` hooks could both
/// see `!exists()` and race on `write()`.
pub fn create_if_missing<F>(
    project_root: &str,
    session_id: &str,
    make_state: F,
) -> std::io::Result<bool>
where
    F: FnOnce() -> ActiveSession,
{
    let dir = buffer_dir();
    let _ = fs::create_dir_all(&dir);
    let _lock = BufferLock::acquire(session_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("could not acquire buffer lock for session {session_id}"),
        )
    })?;
    if read(project_root, session_id).is_some() {
        return Ok(false);
    }
    let state = make_state();
    let path = buffer_path(session_id);
    let json = serde_json::to_string_pretty(&state).map_err(std::io::Error::other)?;
    atomic_write_json(&path, &json)?;
    Ok(true)
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
                    // A session belongs to a project if it originated there
                    // OR routed foreign edits into it (cross-repo capture).
                    let matches_root = state
                        .worktree
                        .as_deref()
                        .is_some_and(|wt| worktree_matches(wt, project_root))
                        || state.foreign_repos.as_ref().is_some_and(|repos| {
                            repos
                                .keys()
                                .any(|root| worktree_matches(root, project_root))
                        });
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
    let _lock = BufferLock::acquire(session_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("could not acquire buffer lock for session {session_id}"),
        )
    })?;
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
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
    use serial_test::serial;

    use super::*;

    fn fresh_buffer_env() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("OOBO_HOME", tmp.path());
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
            resumed_from: None,
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
            foreign_repos: None,
            event_seq: 0,
            started_at: 1,
            updated_at: 1,
            ended_at: None,
        }
    }

    #[test]
    #[serial]
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
        let traversal = sanitize("../../etc/passwd");
        assert!(
            !traversal.contains(".."),
            "must not contain path traversal: {traversal}"
        );
        assert!(
            !traversal.contains('/'),
            "must not contain separator: {traversal}"
        );
        assert_eq!(sanitize("good-id-123"), "good-id-123");
        assert_eq!(sanitize(""), "invalid");
        assert_eq!(sanitize("id_with.dots"), "id_with.dots");
        assert_eq!(sanitize("id:with:colons"), "id_with_colons");
        let sneaky = sanitize("..sneaky");
        assert!(
            !sneaky.starts_with('.'),
            "must not start with dot: {sneaky}"
        );
        assert_eq!(sanitize("."), "invalid");
        let dots = sanitize("...");
        assert!(!dots.contains(".."), "must not contain traversal: {dots}");
    }

    // ── E1 fix: read_modify_write holds lock across RMW cycle ─────────

    #[test]
    #[serial]
    fn test_read_modify_write_updates_state() {
        let _env = fresh_buffer_env();
        let state = mk_state("rmw-test");
        write("", "rmw-test", &state).unwrap();

        let updated = read_modify_write("", "rmw-test", |s| {
            s.current_turn_index = 42;
            s.model = Some("opus".into());
        })
        .unwrap();
        assert!(updated, "should return true when session exists");

        let back = read("", "rmw-test").unwrap();
        assert_eq!(back.current_turn_index, 42);
        assert_eq!(back.model.as_deref(), Some("opus"));
        remove("", "rmw-test");
    }

    #[test]
    #[serial]
    fn test_read_modify_write_returns_false_on_missing_session() {
        let _env = fresh_buffer_env();
        let result = read_modify_write("", "nonexistent-sid", |_| {}).unwrap();
        assert!(!result, "should return false when session doesn't exist");
    }

    #[test]
    #[serial]
    fn test_read_modify_write_with_returns_value() {
        let _env = fresh_buffer_env();
        let mut state = mk_state("rmw-val-test");
        state.current_turn_index = 5;
        write("", "rmw-val-test", &state).unwrap();

        let result = read_modify_write_with("", "rmw-val-test", |s| {
            let old_idx = s.current_turn_index;
            s.current_turn_index += 1;
            old_idx
        })
        .unwrap();
        assert_eq!(result, Some(5), "should return the value from the closure");

        let back = read("", "rmw-val-test").unwrap();
        assert_eq!(back.current_turn_index, 6, "state should be persisted");
        remove("", "rmw-val-test");
    }

    #[test]
    #[serial]
    fn test_read_modify_write_with_returns_none_on_missing() {
        let _env = fresh_buffer_env();
        let result = read_modify_write_with::<_, i64>("", "no-such-session", |_| 999).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    #[serial]
    fn test_read_locked_write_locked_roundtrip() {
        let _env = fresh_buffer_env();
        let state = mk_state("locked-test");
        write("", "locked-test", &state).unwrap();

        let (guard, mut loaded) = read_locked("", "locked-test").unwrap();
        assert_eq!(loaded.session_id, "locked-test");
        loaded.current_turn_index = 99;
        loaded.model = Some("sonnet".into());
        write_locked(&guard, "locked-test", &loaded).unwrap();
        drop(guard);

        let back = read("", "locked-test").unwrap();
        assert_eq!(back.current_turn_index, 99);
        assert_eq!(back.model.as_deref(), Some("sonnet"));
        remove("", "locked-test");
    }

    #[test]
    #[serial]
    fn test_read_locked_returns_none_on_missing() {
        let _env = fresh_buffer_env();
        assert!(read_locked("", "ghost-session").is_none());
    }

    #[test]
    #[serial]
    fn test_lock_file_created_and_released() {
        let _env = fresh_buffer_env();
        let state = mk_state("lock-file-test");
        write("", "lock-file-test", &state).unwrap();

        let lock_path = buffer_dir().join(format!("{}.lock", sanitize("lock-file-test")));

        // Lock file shouldn't exist before we acquire.
        assert!(!lock_path.exists());

        let (guard, _) = read_locked("", "lock-file-test").unwrap();
        // Lock file exists while guard is held.
        assert!(lock_path.exists(), "lock file should exist while held");

        drop(guard);
        // Lock file removed after guard is dropped.
        assert!(
            !lock_path.exists(),
            "lock file should be removed after drop"
        );
        remove("", "lock-file-test");
    }

    #[test]
    #[serial]
    fn test_concurrent_lock_acquisition_serializes() {
        let _env = fresh_buffer_env();
        let state = mk_state("serial-test");
        write("", "serial-test", &state).unwrap();

        // First acquisition succeeds.
        let lock1 = BufferLock::acquire("serial-test");
        assert!(lock1.is_some(), "first lock acquisition should succeed");

        // Second acquisition within the same process fails (lock held).
        let lock2 = BufferLock::acquire("serial-test");
        assert!(
            lock2.is_none(),
            "concurrent lock on same session should fail while held"
        );

        // After releasing first, second should succeed.
        drop(lock1);
        let lock3 = BufferLock::acquire("serial-test");
        assert!(
            lock3.is_some(),
            "lock acquisition should succeed after release"
        );
        drop(lock3);
        remove("", "serial-test");
    }

    #[test]
    #[serial]
    fn test_read_modify_write_blocks_while_locked() {
        let _env = fresh_buffer_env();
        let state = mk_state("block-test");
        write("", "block-test", &state).unwrap();

        // Hold the lock directly.
        let lock = BufferLock::acquire("block-test").unwrap();

        // read_modify_write should fail because lock is held.
        let result = read_modify_write("", "block-test", |s| {
            s.current_turn_index = 999;
        });
        assert!(
            result.is_err(),
            "read_modify_write should fail when lock is held"
        );
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);

        drop(lock);

        // Verify state was NOT modified (the write should have been rejected).
        let back = read("", "block-test").unwrap();
        assert_eq!(
            back.current_turn_index, 0,
            "state must not be modified when lock blocked the write"
        );
        remove("", "block-test");
    }
}
