//! Active-session tracking for hook events.
//!
//! State is persisted through [`crate::hooks::store`], which transparently
//! prefers the SQLite `hook_sessions` table and falls back to a per-session
//! buffer file in `~/.oobo/tmp/hook-buffer/` when no project can be resolved
//! (pre-`git init`) or when the DB is transiently unavailable. Legacy files
//! in `<git-common-dir>/oobo-sessions/*.json` (written by oobo 0.1.x) are
//! read lazily and imported into the DB on first access.
//!
//! All public functions here are thin wrappers that load, mutate, and save
//! an [`ActiveSession`] via the store — they never touch the filesystem
//! directly anymore.

use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::hooks::store;

/// A subagent spawned during an agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentRun {
    pub agent_id: String,
    pub agent_type: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

/// Active session state.
///
/// Lightweight and ephemeral: tracks which agent sessions are active right
/// now. Used by the post-commit hook to link sessions to commits. The
/// `worktree` field enables correct session→commit linking when multiple
/// agents work in parallel via git worktrees.
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
    /// Git blob hashes of files BEFORE the agent's edit (pre-agent state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_agent_snapshots: Option<std::collections::HashMap<String, String>>,
    /// Git blob hashes of files AFTER the agent's last edit (post-agent state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_snapshots: Option<std::collections::HashMap<String, String>>,
    /// Files edited by the agent, accumulated from PostToolUse hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_files: Option<std::collections::HashSet<String>>,
    /// Files read by the agent, accumulated from PostToolUse hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_files: Option<std::collections::HashSet<String>>,
    /// Tool usage counts by tool name (e.g. {"Bash": 12, "Edit": 8, "Read": 15}).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_usage: Option<std::collections::HashMap<String, u32>>,
    /// Number of failed tool calls during this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_failures: Option<u32>,
    /// Recent bash commands executed by the agent (capped at 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bash_commands: Option<Vec<String>>,
    /// Subagents spawned during this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_runs: Option<Vec<SubagentRun>>,
    /// Accumulated thinking time in milliseconds (from afterAgentThought hooks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_duration_ms: Option<u64>,
    /// Number of context compaction events during this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_count: Option<u32>,
    pub started_at: i64,
    pub updated_at: i64,
}

impl ActiveSession {
    fn new(session_id: &str, agent: &str, model: Option<&str>, worktree: Option<String>) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            session_id: session_id.to_string(),
            agent: agent.to_string(),
            model: model.map(|s| s.to_string()),
            worktree,
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
            started_at: now,
            updated_at: now,
        }
    }

    fn bump(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp();
    }
}

/// Resolve the worktree root for the current directory.
/// Returns canonicalized path to avoid macOS `/var` vs `/private/var` mismatches.
fn resolve_worktree(project_root: &str) -> Option<String> {
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

// ── Mutators ──────────────────────────────────────────────────────────

/// Load state, apply `f`, bump `updated_at`, and save.
/// No-op (returns `Ok(())`) if the session doesn't exist.
fn mutate<F>(project_root: &str, session_id: &str, f: F) -> Result<()>
where
    F: FnOnce(&mut ActiveSession),
{
    let Some(mut state) = store::read(project_root, session_id) else {
        return Ok(());
    };
    f(&mut state);
    state.bump();
    store::write(project_root, session_id, &state)?;
    Ok(())
}

/// Create a new active session.
pub fn write_session(
    project_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
) -> Result<()> {
    let worktree = resolve_worktree(project_root);
    let state = ActiveSession::new(session_id, agent, model, worktree);
    store::write(project_root, session_id, &state)?;
    Ok(())
}

/// Ensure a session exists. If `session-start` fired before `git init`,
/// or for any other reason the session isn't tracked yet, create it.
pub fn ensure_session(
    project_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
) -> Result<()> {
    if store::exists(project_root, session_id) {
        return Ok(());
    }
    log_ensure("creating (not found)", session_id);
    write_session(project_root, session_id, agent, model)
}

fn log_ensure(msg: &str, session_id: &str) {
    if let Some(home) = dirs::home_dir() {
        let line = format!(
            "{} ensure_session: {} sid={}\n",
            chrono::Utc::now().to_rfc3339(),
            msg,
            session_id,
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join(".oobo/logs/hooks-debug.log"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

/// Update the timestamp on an existing session (turn ended, session continues).
/// Also stores `transcript_path` if provided by the hook event.
pub fn touch_session(
    project_root: &str,
    session_id: &str,
    transcript_path: Option<&str>,
) -> Result<()> {
    mutate(project_root, session_id, |state| {
        if let Some(tp) = transcript_path {
            if !tp.is_empty() {
                state.transcript_path = Some(tp.to_string());
            }
        }
    })
}

/// Record a file edited by the agent during this session.
pub fn record_edited_file(project_root: &str, session_id: &str, file_path: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        let mut files = state.edited_files.take().unwrap_or_default();
        files.insert(file_path.to_string());
        state.edited_files = Some(files);
    })
}

/// Record a file read by the agent during this session.
pub fn record_read_file(project_root: &str, session_id: &str, file_path: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        let mut files = state.read_files.take().unwrap_or_default();
        files.insert(file_path.to_string());
        state.read_files = Some(files);
    })
}

/// Record a tool call by the agent. Increments the tool_usage count and
/// optionally stores a bash command summary.
pub fn record_tool_use(
    project_root: &str,
    session_id: &str,
    tool_name: &str,
    input_summary: Option<&str>,
) -> Result<()> {
    mutate(project_root, session_id, |state| {
        let mut usage = state.tool_usage.take().unwrap_or_default();
        *usage.entry(tool_name.to_string()).or_insert(0) += 1;
        state.tool_usage = Some(usage);

        if tool_name == "Bash" || tool_name == "Shell" {
            if let Some(cmd) = input_summary {
                let mut cmds = state.bash_commands.take().unwrap_or_default();
                const MAX_BASH_COMMANDS: usize = 50;
                if cmds.len() >= MAX_BASH_COMMANDS {
                    cmds.remove(0);
                }
                cmds.push(cmd.to_string());
                state.bash_commands = Some(cmds);
            }
        }
    })
}

/// Record a failed tool call.
pub fn record_tool_failure(project_root: &str, session_id: &str, tool_name: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        state.tool_failures = Some(state.tool_failures.unwrap_or(0) + 1);
        // PostToolUseFailure fires *instead of* PostToolUse (not in addition),
        // so we count failures in tool_usage to keep the total accurate.
        let mut usage = state.tool_usage.take().unwrap_or_default();
        *usage.entry(tool_name.to_string()).or_insert(0) += 1;
        state.tool_usage = Some(usage);
    })
}

/// Record a subagent spawn.
pub fn record_subagent_start(
    project_root: &str,
    session_id: &str,
    agent_id: &str,
    agent_type: &str,
) -> Result<()> {
    mutate(project_root, session_id, |state| {
        let mut runs = state.subagent_runs.take().unwrap_or_default();
        runs.push(SubagentRun {
            agent_id: agent_id.to_string(),
            agent_type: agent_type.to_string(),
            started_at: chrono::Utc::now().timestamp(),
            ended_at: None,
        });
        state.subagent_runs = Some(runs);
    })
}

/// Record a subagent completing by setting its ended_at timestamp.
pub fn record_subagent_end(project_root: &str, session_id: &str, agent_id: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        if let Some(ref mut runs) = state.subagent_runs {
            for run in runs.iter_mut().rev() {
                if run.agent_id == agent_id && run.ended_at.is_none() {
                    run.ended_at = Some(chrono::Utc::now().timestamp());
                    break;
                }
            }
        }
    })
}

/// Record thinking duration from an afterAgentThought hook.
pub fn record_thinking(project_root: &str, session_id: &str, duration_ms: u64) -> Result<()> {
    mutate(project_root, session_id, |state| {
        let current = state.thinking_duration_ms.unwrap_or(0);
        state.thinking_duration_ms = Some(current + duration_ms);
    })
}

/// Record a context compaction event.
pub fn record_compact(project_root: &str, session_id: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        state.compact_count = Some(state.compact_count.unwrap_or(0) + 1);
    })
}

// ── Readers ───────────────────────────────────────────────────────────

/// Read the accumulated edited_files set from a session's state.
pub fn get_edited_files(project_root: &str, session_id: &str) -> Vec<String> {
    store::read(project_root, session_id)
        .and_then(|s| s.edited_files)
        .map(|s| s.into_iter().collect())
        .unwrap_or_default()
}

/// Read both edited_files and read_files from a session's state.
pub fn get_file_sets(project_root: &str, session_id: &str) -> (Vec<String>, Vec<String>) {
    let Some(state) = store::read(project_root, session_id) else {
        return (Vec::new(), Vec::new());
    };
    let edited = state
        .edited_files
        .map(|s| s.into_iter().collect())
        .unwrap_or_default();
    let read = state
        .read_files
        .map(|s| s.into_iter().collect())
        .unwrap_or_default();
    (edited, read)
}

/// Read and deserialize the active session state, if it exists.
pub fn read_session(project_root: &str, session_id: &str) -> Option<ActiveSession> {
    store::read(project_root, session_id)
}

/// Read the model field from a session's state.
pub fn read_session_model(project_root: &str, session_id: &str) -> Option<String> {
    read_session(project_root, session_id).and_then(|s| s.model)
}

/// Remove a session's state (session ended).
pub fn remove_session(project_root: &str, session_id: &str) {
    store::remove(project_root, session_id);
}

/// List all currently active sessions for a project.
pub fn active_sessions(project_root: &str) -> Vec<ActiveSession> {
    store::list_for_project(project_root)
}

/// List active sessions filtered to only those belonging to the given worktree.
/// Sessions without a worktree field (pre-upgrade) are included in all worktrees
/// for backward compatibility.
pub fn active_sessions_for_worktree(project_root: &str) -> Vec<ActiveSession> {
    let current_wt = resolve_worktree(project_root);
    let all = active_sessions(project_root);

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

// ── Snapshots ─────────────────────────────────────────────────────────

/// Capture the pre-agent file state: snapshot currently dirty files in the
/// worktree. Called on `before-submit-prompt` — any worktree changes at this
/// moment are human edits (the agent hasn't started yet).
pub fn snapshot_pre_agent_state(project_root: &str, session_id: &str) -> Result<()> {
    let snapshots = snapshot_dirty_files(project_root);
    if snapshots.is_none() {
        return Ok(());
    }
    mutate(project_root, session_id, |state| {
        state.pre_agent_snapshots = snapshots;
    })
}

fn snapshot_dirty_files(
    project_root: &str,
) -> Option<std::collections::HashMap<String, String>> {
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

fn hash_object(git: &str, project_root: &str, file: &str) -> Option<String> {
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
    mutate(project_root, session_id, |state| {
        let mut snapshots = state.file_snapshots.take().unwrap_or_default();
        snapshots.extend(new_snapshots);
        state.file_snapshots = Some(snapshots);
    })
}

/// Clean up stale session state older than `max_age_secs`.
pub fn cleanup_stale(project_root: &str, max_age_secs: i64) {
    let now = chrono::Utc::now().timestamp();
    let sessions = store::list_for_project(project_root);
    for s in sessions {
        if now - s.updated_at > max_age_secs {
            store::remove(project_root, &s.session_id);
        }
    }
    store::cleanup_buffer(max_age_secs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    /// All state tests share a single OOBO_HOME env var — serialize them
    /// so that each test's tempdir stays the active home for its duration.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _oobo_home: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("OOBO_HOME", v),
                    None => std::env::remove_var("OOBO_HOME"),
                }
            }
        }
    }

    fn setup() -> TestEnv {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os("OOBO_HOME");
        let oobo_home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("OOBO_HOME", oobo_home.path());
        }
        TestEnv {
            _oobo_home: oobo_home,
            prev,
            _guard: guard,
        }
    }

    fn init_git_repo(root: &Path) {
        std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(root)
            .status();
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(root)
            .status();
    }

    #[test]
    fn test_session_lifecycle() {
        let _env = setup();
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
    fn test_record_and_get_edited_files() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "edit-sess", "claude", None).unwrap();
        assert!(get_edited_files(root_str, "edit-sess").is_empty());

        record_edited_file(root_str, "edit-sess", "src/main.rs").unwrap();
        record_edited_file(root_str, "edit-sess", "src/lib.rs").unwrap();
        record_edited_file(root_str, "edit-sess", "src/main.rs").unwrap(); // duplicate

        let files = get_edited_files(root_str, "edit-sess");
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_record_and_get_read_files() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "read-sess", "claude", None).unwrap();

        record_read_file(root_str, "read-sess", "src/main.rs").unwrap();
        record_read_file(root_str, "read-sess", "src/lib.rs").unwrap();
        record_read_file(root_str, "read-sess", "src/main.rs").unwrap();

        let (_, files) = get_file_sets(root_str, "read-sess");
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_record_edited_file_nonexistent_session_is_noop() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        assert!(record_edited_file(root_str, "no-such-session", "file.rs").is_ok());
        assert!(get_edited_files(root_str, "no-such-session").is_empty());
    }

    #[test]
    fn test_record_tool_use_and_bash_commands() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "tool-sess", "cursor", None).unwrap();

        record_tool_use(root_str, "tool-sess", "Read", Some("/src/main.rs")).unwrap();
        record_tool_use(root_str, "tool-sess", "Read", Some("/src/lib.rs")).unwrap();
        record_tool_use(root_str, "tool-sess", "Bash", Some("ls -la")).unwrap();
        record_tool_use(root_str, "tool-sess", "Shell", Some("cargo build")).unwrap();

        let state = read_session(root_str, "tool-sess").unwrap();
        let usage = state.tool_usage.unwrap();
        assert_eq!(usage.get("Read"), Some(&2));
        assert_eq!(usage.get("Bash"), Some(&1));
        assert_eq!(usage.get("Shell"), Some(&1));
        let cmds = state.bash_commands.unwrap();
        assert_eq!(cmds, vec!["ls -la", "cargo build"]);
    }

    #[test]
    fn test_bash_commands_cap_at_50() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "cap-sess", "claude", None).unwrap();
        for i in 0..60 {
            record_tool_use(root_str, "cap-sess", "Bash", Some(&format!("cmd-{i}"))).unwrap();
        }

        let state = read_session(root_str, "cap-sess").unwrap();
        let cmds = state.bash_commands.unwrap();
        assert_eq!(cmds.len(), 50);
        assert_eq!(cmds[0], "cmd-10");
        assert_eq!(cmds[49], "cmd-59");
    }

    #[test]
    fn test_record_tool_failure() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "fail-sess", "claude", None).unwrap();

        record_tool_failure(root_str, "fail-sess", "Write").unwrap();
        record_tool_failure(root_str, "fail-sess", "Write").unwrap();
        record_tool_failure(root_str, "fail-sess", "Edit").unwrap();

        let state = read_session(root_str, "fail-sess").unwrap();
        assert_eq!(state.tool_failures, Some(3));
        let usage = state.tool_usage.unwrap();
        assert_eq!(usage.get("Write"), Some(&2));
        assert_eq!(usage.get("Edit"), Some(&1));
    }

    #[test]
    fn test_subagent_lifecycle() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "sub-sess", "cursor", None).unwrap();
        record_subagent_start(root_str, "sub-sess", "agent-1", "explore").unwrap();
        record_subagent_start(root_str, "sub-sess", "agent-2", "code").unwrap();
        record_subagent_end(root_str, "sub-sess", "agent-1").unwrap();

        let state = read_session(root_str, "sub-sess").unwrap();
        let runs = state.subagent_runs.unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].agent_id, "agent-1");
        assert!(runs[0].ended_at.is_some());
        assert_eq!(runs[1].agent_id, "agent-2");
        assert!(runs[1].ended_at.is_none());
    }

    #[test]
    fn test_record_thinking_accumulates() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "think-sess", "cursor", None).unwrap();
        record_thinking(root_str, "think-sess", 1500).unwrap();
        record_thinking(root_str, "think-sess", 2500).unwrap();

        let state = read_session(root_str, "think-sess").unwrap();
        assert_eq!(state.thinking_duration_ms, Some(4000));
    }

    #[test]
    fn test_record_compact_increments() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "compact-sess", "cursor", None).unwrap();
        record_compact(root_str, "compact-sess").unwrap();
        record_compact(root_str, "compact-sess").unwrap();
        record_compact(root_str, "compact-sess").unwrap();

        let state = read_session(root_str, "compact-sess").unwrap();
        assert_eq!(state.compact_count, Some(3));
    }

    #[test]
    fn test_read_session_model() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "model-test", "cursor", Some("gpt-4o")).unwrap();
        assert_eq!(
            read_session_model(root_str, "model-test").as_deref(),
            Some("gpt-4o")
        );

        write_session(root_str, "no-model", "cursor", None).unwrap();
        assert!(read_session_model(root_str, "no-model").is_none());
    }

    /// Pre-git-init: project_root is empty, so state lands in the buffer.
    /// Later, once we have a real project root, the DB becomes the primary
    /// but the buffered session is still readable.
    #[test]
    fn test_pre_git_init_buffer_fallback() {
        let _env = setup();
        // First write: no project root → buffer.
        write_session("", "pre-init-sid", "cursor", None).unwrap();

        // Still readable via the empty-root API (buffer hit).
        let state = read_session("", "pre-init-sid");
        assert!(state.is_some());
        assert_eq!(state.unwrap().session_id, "pre-init-sid");

        // Remove it.
        remove_session("", "pre-init-sid");
        assert!(read_session("", "pre-init-sid").is_none());
    }

    /// Legacy `.git/oobo-sessions/<sid>.json` files written by oobo 0.1.x
    /// should be discovered and lazily imported into the DB on first read.
    #[test]
    fn test_legacy_file_lazy_import() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        // Hand-write a legacy session file directly.
        let legacy_dir = root.join(".git").join("oobo-sessions");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_file = legacy_dir.join("legacy-sid.json");
        let legacy = ActiveSession::new("legacy-sid", "claude", None, None);
        std::fs::write(
            &legacy_file,
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        // First read imports it into the DB and deletes the legacy file.
        let state = read_session(root_str, "legacy-sid");
        assert!(state.is_some());
        assert_eq!(state.unwrap().session_id, "legacy-sid");
        assert!(
            !legacy_file.exists(),
            "legacy file should be cleaned up after import"
        );

        // Second read comes from the DB (legacy file is gone).
        let state = read_session(root_str, "legacy-sid");
        assert!(state.is_some());
    }

    #[test]
    fn test_legacy_file_readable_in_active_sessions() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        let legacy_dir = root.join(".git").join("oobo-sessions");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy = ActiveSession::new("sess-legacy", "claude", None, None);
        std::fs::write(
            legacy_dir.join("sess-legacy.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let all = active_sessions(root_str);
        assert!(all.iter().any(|s| s.session_id == "sess-legacy"));
    }

    #[test]
    fn test_write_to_db_clears_legacy_file() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        // Create a legacy file, then touch the session (should promote).
        let legacy_dir = root.join(".git").join("oobo-sessions");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy = ActiveSession::new("drain-sid", "claude", None, None);
        let legacy_file = legacy_dir.join("drain-sid.json");
        std::fs::write(
            &legacy_file,
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        // Mutating path goes: read (imports + drains) → mutate → write.
        touch_session(root_str, "drain-sid", None).unwrap();
        assert!(!legacy_file.exists());
        assert!(read_session(root_str, "drain-sid").is_some());
    }
}
