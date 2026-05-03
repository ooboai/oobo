//! Active-session tracking for hook events.
//!
//! State is persisted through [`crate::hooks::store`], which uses per-session
//! JSON buffer files in `~/.oobo/tmp/hook-buffer/`. Legacy files in
//! `<git-common-dir>/oobo-sessions/*.json` (written by oobo 0.1.x) are read
//! lazily during session discovery.
//!
//! All public functions here are thin wrappers that load, mutate, and save
//! an [`ActiveSession`] via the store — they never touch the filesystem
//! directly anymore.

mod snapshots;
mod turns;

pub use snapshots::{
    cleanup_stale, record_post_edit_file, snapshot_pre_agent_state, snapshot_pre_edit_file,
    snapshot_session_files,
};
pub use turns::{finish_turn, mark_restored_from};

use serde::{Deserialize, Serialize};

use crate::core::turn::{TurnHookEvent, TurnToolCall};
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

/// A single file-edit event capturing the git blob hash before and after an AI
/// tool call. Together, the chain of `FileEditPair`s for a file gives us
/// per-edit attribution granularity (similar to git-ai's checkpoint system).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditPair {
    /// Git blob hash of the file BEFORE the edit (from preToolUse).
    pub pre_blob: String,
    /// Git blob hash of the file AFTER the edit (from postToolUse).
    pub post_blob: String,
    /// Tool that made the edit (e.g. "Write", "Edit", "StrReplace").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Unix timestamp when the edit occurred.
    pub timestamp: i64,
}

/// A timestamped file access event for causality analysis.
/// Records when a session read or wrote a specific file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEvent {
    pub path: String,
    /// "read" or "write"
    pub action: String,
    /// Unix timestamp (seconds) when the event occurred.
    pub timestamp: i64,
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
    /// Timestamped per-file events for causality analysis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_events: Option<Vec<FileEvent>>,
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
    /// Agent turn/loop count reported at session stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u32>,
    /// Total context tokens used by the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Context window size configured for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_size: Option<u64>,
    /// 0-based turn index for Git-backed turn snapshots.
    #[serde(default)]
    pub current_turn_index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_hook_events: Option<Vec<TurnHookEvent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_tool_calls: Option<Vec<TurnToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_snapshot_id: Option<String>,
    /// Pending pre-edit blob hashes awaiting pairing with post-edit blobs.
    /// Keyed by relative file path. Set in preToolUse, consumed in postToolUse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_edit_pending: Option<std::collections::HashMap<String, String>>,
    /// Ordered chain of pre/post blob pairs per file, giving per-edit
    /// attribution granularity. Keyed by relative file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_edit_chain: Option<std::collections::HashMap<String, Vec<FileEditPair>>>,
    pub started_at: i64,
    pub updated_at: i64,
}

impl ActiveSession {
    fn new(session_id: &str, agent: &str, model: Option<&str>, worktree: Option<String>) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            session_id: session_id.to_string(),
            agent: agent.to_string(),
            model: model.map(std::string::ToString::to_string),
            worktree,
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
            started_at: now,
            updated_at: now,
        }
    }

    fn bump(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp();
    }
}

// ── Mutators ──────────────────────────────────────────────────────────

/// Load state, apply `f`, bump `updated_at`, and save.
/// No-op (returns `Ok(())`) if the session doesn't exist.
pub(super) fn mutate<F>(project_root: &str, session_id: &str, f: F) -> Result<()>
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
#[tracing::instrument(skip_all, fields(session_id, agent))]
pub fn write_session(
    project_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
) -> Result<()> {
    tracing::info!(session_id, agent, "session start");
    let worktree = snapshots::resolve_worktree(project_root);
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
    tracing::debug!(session_id, msg, "ensure_session");
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

pub fn start_turn(project_root: &str, session_id: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        if state.current_turn_started_at.is_some() {
            return;
        }
        if state.last_turn_snapshot_id.is_some() {
            state.current_turn_index += 1;
        }
        state.current_turn_started_at = Some(chrono::Utc::now().timestamp());
        state.current_turn_hook_events = Some(Vec::new());
        state.current_turn_tool_calls = Some(Vec::new());
    })
}

pub fn record_hook_event(
    project_root: &str,
    session_id: &str,
    event_name: &str,
    payload: Option<serde_json::Value>,
) -> Result<()> {
    mutate(project_root, session_id, |state| {
        if state.current_turn_started_at.is_none() {
            state.current_turn_started_at = Some(chrono::Utc::now().timestamp());
        }
        let mut events = state.current_turn_hook_events.take().unwrap_or_default();
        events.push(TurnHookEvent {
            name: event_name.to_string(),
            observed_at: chrono::Utc::now().timestamp(),
            payload,
        });
        state.current_turn_hook_events = Some(events);
    })
}

pub fn record_tool_call(
    project_root: &str,
    session_id: &str,
    tool_name: &str,
    input: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    failed: bool,
) -> Result<()> {
    mutate(project_root, session_id, |state| {
        if state.current_turn_started_at.is_none() {
            state.current_turn_started_at = Some(chrono::Utc::now().timestamp());
        }
        let mut calls = state.current_turn_tool_calls.take().unwrap_or_default();
        calls.push(TurnToolCall {
            name: tool_name.to_string(),
            observed_at: chrono::Utc::now().timestamp(),
            input,
            output,
            failed,
        });
        state.current_turn_tool_calls = Some(calls);
    })
}

/// Record a file edited by the agent during this session.
pub fn record_edited_file(project_root: &str, session_id: &str, file_path: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        let mut files = state.edited_files.take().unwrap_or_default();
        files.insert(file_path.to_string());
        state.edited_files = Some(files);
        let mut events = state.file_events.take().unwrap_or_default();
        events.push(FileEvent {
            path: file_path.to_string(),
            action: "write".into(),
            timestamp: chrono::Utc::now().timestamp(),
        });
        state.file_events = Some(events);
    })
}

/// Record a file read by the agent during this session.
pub fn record_read_file(project_root: &str, session_id: &str, file_path: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        let mut files = state.read_files.take().unwrap_or_default();
        files.insert(file_path.to_string());
        state.read_files = Some(files);
        let mut events = state.file_events.take().unwrap_or_default();
        events.push(FileEvent {
            path: file_path.to_string(),
            action: "read".into(),
            timestamp: chrono::Utc::now().timestamp(),
        });
        state.file_events = Some(events);
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

/// Update session with metrics reported by hook events (e.g. stop payload).
pub fn update_session_metrics(
    project_root: &str,
    session_id: &str,
    loop_count: Option<u32>,
    context_tokens: Option<u64>,
    context_window_size: Option<u64>,
) -> Result<()> {
    if loop_count.is_none() && context_tokens.is_none() && context_window_size.is_none() {
        return Ok(());
    }
    mutate(project_root, session_id, |state| {
        if let Some(lc) = loop_count {
            state.turn_count = Some(lc);
        }
        if let Some(ct) = context_tokens {
            state.context_tokens = Some(ct);
        }
        if let Some(cw) = context_window_size {
            state.context_window_size = Some(cw);
        }
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

/// Read timestamped file events for a session.
pub fn get_file_events(project_root: &str, session_id: &str) -> Vec<FileEvent> {
    store::read(project_root, session_id)
        .and_then(|s| s.file_events)
        .unwrap_or_default()
}

/// Read and deserialize the active session state, if it exists.
#[cfg(test)]
fn read_session(project_root: &str, session_id: &str) -> Option<ActiveSession> {
    store::read(project_root, session_id)
}

/// Read the model field from a session's state.
#[cfg(test)]
pub fn read_session_model(project_root: &str, session_id: &str) -> Option<String> {
    read_session(project_root, session_id).and_then(|s| s.model)
}

/// Remove a session's state (session ended).
pub fn remove_session(project_root: &str, session_id: &str) {
    tracing::info!(session_id, "session end");
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
    let current_wt = snapshots::resolve_worktree(project_root);
    let all = active_sessions(project_root);

    match current_wt {
        Some(wt) => all
            .into_iter()
            .filter(|s| match &s.worktree {
                Some(session_wt) => {
                    let canonical = std::fs::canonicalize(session_wt).map_or_else(|_| session_wt.clone(), |p| p.to_string_lossy().to_string());
                    canonical == wt
                }
                None => true,
            })
            .collect(),
        None => all,
    }
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
        let guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
    fn test_finish_turn_writes_git_snapshot() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "turn-sess", "cursor", Some("claude")).unwrap();
        start_turn(root_str, "turn-sess").unwrap();
        record_hook_event(
            root_str,
            "turn-sess",
            "before-submit-prompt",
            Some(serde_json::json!({"prompt": "hello"})),
        )
        .unwrap();

        std::fs::write(root.join("answer.txt"), "world\n").unwrap();
        record_edited_file(root_str, "turn-sess", "answer.txt").unwrap();
        snapshot_session_files(root_str, "turn-sess", &["answer.txt".to_string()]).unwrap();
        record_tool_call(
            root_str,
            "turn-sess",
            "Write",
            Some(serde_json::json!({"file_path": "answer.txt"})),
            None,
            false,
        )
        .unwrap();

        let turn_id = finish_turn(root_str, "turn-sess", "cursor", Some("claude"), None)
            .unwrap()
            .unwrap();
        let snapshots = crate::git::turns::list_turn_snapshots(root_str);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, turn_id);
        assert_eq!(snapshots[0].files[0].path, "answer.txt");
        assert_eq!(snapshots[0].memory.tool_calls[0].name, "Write");

        let state = read_session(root_str, "turn-sess").unwrap();
        assert_eq!(
            state.last_turn_snapshot_id.as_deref(),
            Some(turn_id.as_str())
        );
        assert!(state.current_turn_started_at.is_none());
    }

    #[test]
    fn test_finish_turn_prefers_current_turn_file_paths() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "turn-files", "cursor", None).unwrap();
        start_turn(root_str, "turn-files").unwrap();

        std::fs::write(root.join("old.txt"), "old\n").unwrap();
        std::fs::write(root.join("current.txt"), "current\n").unwrap();

        // Session-level edited files are cumulative and may include prior work.
        record_edited_file(root_str, "turn-files", "old.txt").unwrap();
        record_edited_file(root_str, "turn-files", "current.txt").unwrap();
        snapshot_session_files(
            root_str,
            "turn-files",
            &["old.txt".to_string(), "current.txt".to_string()],
        )
        .unwrap();

        // The current turn memory is narrower and should drive TurnSnapshot.files.
        record_tool_call(
            root_str,
            "turn-files",
            "Write",
            Some(serde_json::json!({"file_path": "current.txt"})),
            None,
            false,
        )
        .unwrap();

        let turn_id = finish_turn(root_str, "turn-files", "cursor", None, None)
            .unwrap()
            .unwrap();
        let snapshot = crate::git::turns::read_turn_snapshot(root_str, &turn_id).unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].path, "current.txt");
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
    /// Legacy `.git/oobo-sessions/<sid>.json` files should be readable
    /// as a fallback when no buffer file exists.
    #[test]
    fn test_legacy_file_lazy_import() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        let legacy_dir = root.join(".git").join("oobo-sessions");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_file = legacy_dir.join("legacy-sid.json");
        let legacy = ActiveSession::new("legacy-sid", "claude", None, None);
        std::fs::write(&legacy_file, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        let state = read_session(root_str, "legacy-sid");
        assert!(state.is_some());
        assert_eq!(state.unwrap().session_id, "legacy-sid");

        // Legacy file stays on disk (read-only fallback, no DB drain).
        assert!(legacy_file.exists());
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
    fn test_snapshot_pre_edit_file_new_file() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "pre-new", "cursor", None).unwrap();
        snapshot_pre_edit_file(root_str, "pre-new", "does-not-exist.txt").unwrap();

        let state = read_session(root_str, "pre-new").unwrap();
        let pending = state.pre_edit_pending.unwrap();
        assert_eq!(
            pending.get("does-not-exist.txt").unwrap(),
            "0000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn test_pre_post_edit_pairing() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        std::fs::write(root.join("edit.txt"), "original content\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "edit.txt"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let pre_hash_out = std::process::Command::new("git")
            .args(["hash-object", "-w", "edit.txt"])
            .current_dir(root)
            .stdout(std::process::Stdio::piped())
            .output()
            .unwrap();
        let pre_hash = String::from_utf8_lossy(&pre_hash_out.stdout)
            .trim()
            .to_string();

        write_session(root_str, "pair-sess", "cursor", None).unwrap();
        snapshot_pre_edit_file(root_str, "pair-sess", "edit.txt").unwrap();

        let state = read_session(root_str, "pair-sess").unwrap();
        assert_eq!(
            state.pre_edit_pending.as_ref().unwrap().get("edit.txt").unwrap(),
            &pre_hash
        );

        std::fs::write(root.join("edit.txt"), "modified content\n").unwrap();
        record_post_edit_file(root_str, "pair-sess", "edit.txt", Some("Write")).unwrap();

        let state = read_session(root_str, "pair-sess").unwrap();

        let chain = state.file_edit_chain.as_ref().unwrap();
        let pairs = chain.get("edit.txt").unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].pre_blob, pre_hash);
        assert_ne!(pairs[0].pre_blob, pairs[0].post_blob);
        assert_eq!(pairs[0].tool_name.as_deref(), Some("Write"));

        let pending = state.pre_edit_pending.as_ref().unwrap();
        assert!(!pending.contains_key("edit.txt"));

        let snapshots = state.file_snapshots.as_ref().unwrap();
        assert_eq!(snapshots.get("edit.txt").unwrap(), &pairs[0].post_blob);
    }

    #[test]
    fn test_pre_post_edit_chain_multiple_edits() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        std::fs::write(root.join("multi.txt"), "v0\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "multi.txt"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        write_session(root_str, "multi-sess", "cursor", None).unwrap();

        for i in 1..=3 {
            snapshot_pre_edit_file(root_str, "multi-sess", "multi.txt").unwrap();
            std::fs::write(root.join("multi.txt"), format!("v{i}\n")).unwrap();
            record_post_edit_file(root_str, "multi-sess", "multi.txt", Some("Edit")).unwrap();
        }

        let state = read_session(root_str, "multi-sess").unwrap();
        let chain = state.file_edit_chain.as_ref().unwrap();
        let pairs = chain.get("multi.txt").unwrap();
        assert_eq!(pairs.len(), 3);

        for i in 0..2 {
            assert_eq!(
                pairs[i].post_blob, pairs[i + 1].pre_blob,
                "post_blob of pair {i} should equal pre_blob of pair {}",
                i + 1
            );
        }
    }

    #[test]
    fn test_pre_edit_without_post_does_not_crash() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        std::fs::write(root.join("orphan.txt"), "data\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "orphan.txt"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        write_session(root_str, "orphan-sess", "cursor", None).unwrap();
        start_turn(root_str, "orphan-sess").unwrap();
        snapshot_pre_edit_file(root_str, "orphan-sess", "orphan.txt").unwrap();

        let state = read_session(root_str, "orphan-sess").unwrap();
        assert!(state.pre_edit_pending.is_some());
        assert!(state.file_edit_chain.is_none());

        record_edited_file(root_str, "orphan-sess", "orphan.txt").unwrap();
        snapshot_session_files(root_str, "orphan-sess", &["orphan.txt".to_string()]).unwrap();
        let _ = finish_turn(root_str, "orphan-sess", "cursor", None, None).unwrap();

        let state = read_session(root_str, "orphan-sess").unwrap();
        assert!(state.pre_edit_pending.is_none());
        assert!(state.file_edit_chain.is_none());
    }

    #[test]
    fn test_same_blob_pre_post_skipped() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        std::fs::write(root.join("noop.txt"), "unchanged\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "noop.txt"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        write_session(root_str, "noop-sess", "cursor", None).unwrap();
        snapshot_pre_edit_file(root_str, "noop-sess", "noop.txt").unwrap();
        // Don't modify the file — call record_post_edit_file with the same content.
        record_post_edit_file(root_str, "noop-sess", "noop.txt", Some("Write")).unwrap();

        let state = read_session(root_str, "noop-sess").unwrap();
        assert!(
            state.file_edit_chain.is_none(),
            "identical pre/post blobs should not create an edit pair"
        );
    }

    #[test]
    fn test_touch_promotes_legacy_to_buffer() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        let legacy_dir = root.join(".git").join("oobo-sessions");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy = ActiveSession::new("drain-sid", "claude", None, None);
        let legacy_file = legacy_dir.join("drain-sid.json");
        std::fs::write(&legacy_file, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        touch_session(root_str, "drain-sid", None).unwrap();
        assert!(read_session(root_str, "drain-sid").is_some());
    }
}
