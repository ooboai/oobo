//! Active-session tracking for hook events.
//!
//! State is persisted through [`crate::hooks::store`], which uses per-session
//! JSON buffer files in `~/.oobo/tmp/hook-buffer/`. Legacy files in
//! `<git-common-dir>/oobo-sessions/*.json` (written by oobo 0.1.x) are read
//! lazily during session discovery.
//!
//! All public functions here are thin wrappers that load, mutate, and save
//! an [`ActiveSession`] via the store  --  they never touch the filesystem
//! directly anymore.

mod snapshots;
mod turns;

pub use snapshots::{
    cleanup_stale, record_post_edit_file, record_post_edit_file_in_repo, snapshot_files_in_repo,
    snapshot_pre_agent_state, snapshot_pre_edit_file, snapshot_pre_edit_file_in_repo,
    snapshot_session_files,
};
pub use turns::{finish_foreign_turns, finish_turn, mark_restored_from};

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
    /// Session-monotonic sequence number  --  total order of captured edits
    /// within one session, immune to same-second timestamp ties.
    #[serde(default)]
    pub seq: i64,
    /// Microsecond-resolution capture time, for ordering edits across
    /// concurrent sessions where `seq` counters are independent.
    #[serde(default)]
    pub timestamp_us: i64,
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

/// Capture state for edits a session makes in a repository *other than*
/// its origin. Keyed by the foreign repo's worktree root in
/// [`ActiveSession::foreign_repos`]. All paths are relative to that repo's
/// root, and all blob hashes live in that repo's object database  --  so
/// the foreign repo is fully self-contained for attribution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoCapture {
    /// Files edited in this repo, cumulative for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_files: Option<std::collections::HashSet<String>>,
    /// Timestamped per-file events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_events: Option<Vec<FileEvent>>,
    /// Pre-edit blob hashes awaiting pairing with post-edit blobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_edit_pending: Option<std::collections::HashMap<String, String>>,
    /// Ordered chain of pre/post blob pairs per file (current turn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_edit_chain: Option<std::collections::HashMap<String, Vec<FileEditPair>>>,
    /// Latest post-edit blob per file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_snapshots: Option<std::collections::HashMap<String, String>>,
    /// Files edited in this repo during the current turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_files: Option<std::collections::HashSet<String>>,
    /// When the current turn first touched this repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_started_at: Option<i64>,
    /// 0-based turn index for this repo's turn snapshots (independent of
    /// the origin's turn index  --  each repo only counts turns that
    /// touched it).
    #[serde(default)]
    pub turn_index: i64,
    /// Last turn snapshot written to this repo, for parent chaining.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_snapshot_id: Option<String>,
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
    /// Native session id this session explicitly resumed, when the tool
    /// reports it at session-start. Explicit signal only — never inferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
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
    /// Capture state for edits routed to repositories other than the
    /// origin, keyed by the foreign repo's worktree root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_repos: Option<std::collections::HashMap<String, RepoCapture>>,
    /// Monotonic counter for captured edit events in this session.
    /// Each hook invocation is a separate short-lived process, so the
    /// counter lives in the persisted state, advanced under the same
    /// read-modify-write as the edit it stamps.
    #[serde(default)]
    pub event_seq: i64,
    pub started_at: i64,
    pub updated_at: i64,
    /// Timestamp when the session ended. `None` means still active.
    /// Ended sessions are kept around so post-commit can still read their
    /// snapshots; `cleanup_stale` garbage-collects them after 24 h.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
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
            started_at: now,
            updated_at: now,
            ended_at: None,
        }
    }

    fn bump(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Get-or-create the capture bucket for a foreign repo.
    pub fn foreign_capture_mut(&mut self, repo_root: &str) -> &mut RepoCapture {
        self.foreign_repos
            .get_or_insert_with(Default::default)
            .entry(repo_root.to_string())
            .or_default()
    }
}

// ── Mutators ──────────────────────────────────────────────────────────

/// Load state, apply `f`, bump `updated_at`, and save.
/// No-op (returns `Ok(())`) if the session doesn't exist.
/// Holds the buffer lock across the full read-modify-write cycle.
pub(super) fn mutate<F>(project_root: &str, session_id: &str, f: F) -> Result<()>
where
    F: FnOnce(&mut ActiveSession),
{
    store::read_modify_write(project_root, session_id, |state| {
        f(state);
        state.bump();
    })?;
    Ok(())
}

/// Batched session mutations  --  one disk read, one disk write, N mutations.
///
/// Hook events often apply 3–6 mutations to the same session. Without
/// batching, each mutation does a full JSON round-trip (read + deserialize +
/// serialize + write). `SessionBatch` amortizes this to a single round-trip.
/// Holds the buffer lock from open to flush, preventing concurrent RMW races.
pub struct SessionBatch {
    session_id: String,
    state: ActiveSession,
    guard: store::BufferGuard,
}

impl SessionBatch {
    /// Ensure a session exists, then load it for batched mutation.
    /// Acquires the buffer lock, which is held until `flush` is called.
    pub fn open(
        project_root: &str,
        session_id: &str,
        agent: &str,
        model: Option<&str>,
    ) -> Result<Self> {
        ensure_session(project_root, session_id, agent, model)?;
        let (guard, state) = store::read_locked(project_root, session_id).ok_or_else(|| {
            crate::error::OoboError::Other(format!("session '{session_id}' vanished after ensure"))
        })?;
        Ok(Self {
            session_id: session_id.to_string(),
            state,
            guard,
        })
    }

    pub fn state(&self) -> &ActiveSession {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ActiveSession {
        &mut self.state
    }

    /// Flush all accumulated mutations to disk in a single write.
    /// The buffer lock is released when this `SessionBatch` is dropped.
    pub fn flush(mut self) -> Result<()> {
        self.state.bump();
        store::write_locked(&self.guard, &self.session_id, &self.state)?;
        Ok(())
    }
}

/// Start a session — or RESUME it when the same native session id comes
/// back (tool resume, reopened window). A returning id is a continuation
/// of one long session, never a reset: blind re-creation would wipe turn
/// bookkeeping (`current_turn_index`, `last_turn_snapshot_id`, edit
/// chains), making every later turn collide with turn 0 — and the v2
/// store's turn immutability would then silently drop their provenance.
#[tracing::instrument(skip_all, fields(session_id, agent))]
pub fn write_session(
    project_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
) -> Result<()> {
    // Try to create-if-missing under the buffer lock to prevent the
    // check-then-act race where two concurrent session-start hooks both
    // see !exists() and race on write(), with the last writer wiping
    // turn bookkeeping from the first.
    let worktree = snapshots::resolve_worktree(project_root);
    let created = store::create_if_missing(project_root, session_id, || {
        tracing::info!(session_id, agent, "session start");
        ActiveSession::new(session_id, agent, model, worktree)
    })?;
    if !created {
        tracing::info!(session_id, agent, "session resume (same native id)");
        store::read_modify_write(project_root, session_id, |state| {
            state.ended_at = None;
            if model.is_some() {
                state.model = model.map(std::string::ToString::to_string);
            }
            state.bump();
        })?;
    }
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
        // The index is NOT advanced here: finishing a turn consumes its
        // index (same invariant as foreign captures), so turns stay
        // sequential even for tools that never fire a turn-start hook.
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
        const MAX_HOOK_EVENTS: usize = 200;
        if events.len() >= MAX_HOOK_EVENTS {
            events.drain(..=events.len() - MAX_HOOK_EVENTS);
        }
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
        const MAX_TOOL_CALLS: usize = 200;
        if calls.len() >= MAX_TOOL_CALLS {
            calls.drain(..=calls.len() - MAX_TOOL_CALLS);
        }
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

        if matches!(tool_name, "Bash" | "Shell") {
            if let Some(cmd) = input_summary {
                let mut cmds = state.bash_commands.take().unwrap_or_default();
                const MAX_BASH_COMMANDS: usize = 50;
                if cmds.len() >= MAX_BASH_COMMANDS {
                    cmds.drain(..=cmds.len() - MAX_BASH_COMMANDS);
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

/// Record the native session id this session explicitly resumed
/// (tool-reported at session-start; never inferred).
pub fn record_resumed_from(project_root: &str, session_id: &str, prior: &str) -> Result<()> {
    mutate(project_root, session_id, |state| {
        state.resumed_from = Some(prior.to_string());
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

/// Mark a session as ended. The state is kept so that a subsequent
/// `post-commit` hook can still read snapshots for accurate attribution.
/// Actual deletion happens via `cleanup_stale` (called on every post-commit
/// with a 24 h TTL).
pub fn remove_session(project_root: &str, session_id: &str) {
    tracing::info!(session_id, "session end (marking ended, keeping state)");
    let _ = mutate(project_root, session_id, |state| {
        state.ended_at = Some(chrono::Utc::now().timestamp());
    });
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
                    let canonical = std::fs::canonicalize(session_wt).map_or_else(
                        |_| session_wt.clone(),
                        |p| crate::utils::normalize_win_path(&p.to_string_lossy()).to_string(),
                    );
                    canonical == wt
                }
                None => true,
            })
            .collect(),
        None => all,
    }
}

/// Builders for other modules' tests (claim, worker).
#[cfg(test)]
#[allow(clippy::implicit_hasher)]
pub mod test_support {
    use super::*;

    pub fn mk_session_with(
        session_id: &str,
        worktree: &str,
        chain: Option<std::collections::HashMap<String, Vec<FileEditPair>>>,
        foreign: Option<std::collections::HashMap<String, RepoCapture>>,
    ) -> ActiveSession {
        let mut s = ActiveSession::new(session_id, "claude", None, Some(worktree.to_string()));
        s.file_edit_chain = chain;
        s.foreign_repos = foreign;
        s
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;
    use std::path::Path;

    struct TestEnv {
        _oobo_home: tempfile::TempDir,
    }

    fn setup() -> TestEnv {
        let oobo_home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("OOBO_HOME", oobo_home.path());
        }
        TestEnv {
            _oobo_home: oobo_home,
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
    #[serial]
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
        assert_eq!(sessions.len(), 1, "ended session should still be readable");
        assert!(
            sessions[0].ended_at.is_some(),
            "session should be marked as ended"
        );
    }

    #[test]
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
    fn test_pre_git_init_buffer_fallback() {
        let _env = setup();
        // First write: no project root → buffer.
        write_session("", "pre-init-sid", "cursor", None).unwrap();

        // Still readable via the empty-root API (buffer hit).
        let state = read_session("", "pre-init-sid");
        assert!(state.is_some());
        assert_eq!(state.unwrap().session_id, "pre-init-sid");

        // End it  --  state is kept but marked as ended.
        remove_session("", "pre-init-sid");
        let ended = read_session("", "pre-init-sid");
        assert!(ended.is_some(), "ended session should still be readable");
        assert!(ended.unwrap().ended_at.is_some());
    }

    /// Legacy `.git/oobo-sessions/<sid>.json` files written by oobo 0.1.x
    /// Legacy `.git/oobo-sessions/<sid>.json` files should be readable
    /// as a fallback when no buffer file exists.
    #[test]
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
            state
                .pre_edit_pending
                .as_ref()
                .unwrap()
                .get("edit.txt")
                .unwrap(),
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
    #[serial]
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
                pairs[i].post_blob,
                pairs[i + 1].pre_blob,
                "post_blob of pair {i} should equal pre_blob of pair {}",
                i + 1
            );
        }
    }

    #[test]
    #[serial]
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
    #[serial]
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
        // Don't modify the file  --  call record_post_edit_file with the same content.
        record_post_edit_file(root_str, "noop-sess", "noop.txt", Some("Write")).unwrap();

        let state = read_session(root_str, "noop-sess").unwrap();
        assert!(
            state.file_edit_chain.is_none(),
            "identical pre/post blobs should not create an edit pair"
        );
    }

    #[test]
    #[serial]
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

    #[test]
    #[serial]
    fn test_ended_session_preserves_snapshots() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        // Create a file so git can produce a blob hash.
        let file = root.join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        write_session(root_str, "snap-sess", "claude", None).unwrap();
        snapshot_pre_edit_file(root_str, "snap-sess", "main.rs").unwrap();
        std::fs::write(&file, "fn main() { println!(\"hello\"); }").unwrap();
        record_post_edit_file(root_str, "snap-sess", "main.rs", Some("Write")).unwrap();

        let before = read_session(root_str, "snap-sess").unwrap();
        assert!(before.file_snapshots.is_some());
        assert!(before.ended_at.is_none());
        let snapshot_count = before.file_snapshots.as_ref().unwrap().len();

        // End the session  --  snapshots must survive.
        remove_session(root_str, "snap-sess");

        let after = read_session(root_str, "snap-sess").unwrap();
        assert!(after.ended_at.is_some());
        assert!(after.file_snapshots.is_some());
        assert_eq!(
            after.file_snapshots.as_ref().unwrap().len(),
            snapshot_count,
            "file_snapshots must be preserved after session end"
        );
        assert!(
            after.file_edit_chain.is_some(),
            "edit chain must survive session end"
        );
    }

    #[test]
    #[serial]
    fn test_cleanup_stale_respects_ended_grace_period() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "ended-recent", "claude", None).unwrap();
        write_session(root_str, "ended-old", "claude", None).unwrap();
        write_session(root_str, "still-active", "claude", None).unwrap();

        // End both sessions.
        remove_session(root_str, "ended-recent");
        remove_session(root_str, "ended-old");

        // Backdate the "old" ended session's ended_at to 2 hours ago.
        let _ = mutate(root_str, "ended-old", |state| {
            state.ended_at = Some(chrono::Utc::now().timestamp() - 7200);
        });

        cleanup_stale(root_str, 86400);

        assert!(
            read_session(root_str, "ended-recent").is_some(),
            "recently ended session should survive cleanup"
        );
        assert!(
            read_session(root_str, "ended-old").is_none(),
            "session ended 2h ago should be cleaned up (1h grace)"
        );
        assert!(
            read_session(root_str, "still-active").is_some(),
            "active session should survive cleanup"
        );
    }

    // ── Cross-repo capture (session in X edits repo Y) ─────────────────

    /// Returns (origin_dir, foreign_dir) as initialized git repos.
    fn two_repos() -> (tempfile::TempDir, tempfile::TempDir) {
        let origin = tempfile::tempdir().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        init_git_repo(origin.path());
        init_git_repo(foreign.path());
        (origin, foreign)
    }

    fn canon(p: &Path) -> String {
        let s = std::fs::canonicalize(p)
            .unwrap()
            .to_string_lossy()
            .to_string();
        crate::utils::normalize_win_path(&s).to_string()
    }

    #[test]
    #[serial]
    fn test_foreign_pre_post_edit_pairing() {
        let _env = setup();
        let (origin, foreign) = two_repos();
        let origin_root = canon(origin.path());
        let foreign_root = canon(foreign.path());

        std::fs::write(foreign.path().join("api.rs"), "v1\n").unwrap();

        write_session(&origin_root, "x-sess", "claude", None).unwrap();
        snapshot_pre_edit_file_in_repo(&origin_root, "x-sess", &foreign_root, "api.rs").unwrap();
        std::fs::write(foreign.path().join("api.rs"), "v2\n").unwrap();
        record_post_edit_file_in_repo(
            &origin_root,
            "x-sess",
            &foreign_root,
            "api.rs",
            Some("Edit"),
        )
        .unwrap();

        let state = read_session(&origin_root, "x-sess").unwrap();
        let capture = state
            .foreign_repos
            .as_ref()
            .and_then(|m| m.get(&foreign_root))
            .expect("foreign repo capture bucket should exist");
        let pairs = capture
            .file_edit_chain
            .as_ref()
            .and_then(|c| c.get("api.rs"))
            .expect("edit chain for foreign file");
        assert_eq!(pairs.len(), 1);
        assert_ne!(pairs[0].pre_blob, pairs[0].post_blob);

        // Both blobs must be retrievable from the FOREIGN repo's odb  --
        // self-contained attribution.
        for blob in [&pairs[0].pre_blob, &pairs[0].post_blob] {
            let ok = std::process::Command::new("git")
                .args(["cat-file", "-e", blob])
                .current_dir(foreign.path())
                .status()
                .unwrap()
                .success();
            assert!(ok, "blob {blob} must exist in the foreign repo's odb");
        }

        // Origin state must NOT have the foreign file in its own chain.
        assert!(state.file_edit_chain.is_none());
    }

    #[test]
    #[serial]
    fn test_finish_foreign_turns_writes_snapshot_in_foreign_repo() {
        let _env = setup();
        let (origin, foreign) = two_repos();
        let origin_root = canon(origin.path());
        let foreign_root = canon(foreign.path());

        std::fs::write(foreign.path().join("handler.rs"), "a\n").unwrap();
        write_session(&origin_root, "xr-sess", "claude", Some("opus")).unwrap();

        // Turn 1: edit the foreign file.
        snapshot_pre_edit_file_in_repo(&origin_root, "xr-sess", &foreign_root, "handler.rs")
            .unwrap();
        std::fs::write(foreign.path().join("handler.rs"), "b\n").unwrap();
        record_post_edit_file_in_repo(
            &origin_root,
            "xr-sess",
            &foreign_root,
            "handler.rs",
            Some("Edit"),
        )
        .unwrap();
        // Simulate what after-tool-use does for foreign edits.
        let _ = mutate(&origin_root, "xr-sess", |state| {
            let capture = state.foreign_capture_mut(&foreign_root);
            capture.turn_started_at = Some(chrono::Utc::now().timestamp());
            capture
                .edited_files
                .get_or_insert_with(Default::default)
                .insert("handler.rs".into());
            capture
                .turn_files
                .get_or_insert_with(Default::default)
                .insert("handler.rs".into());
        });

        let written =
            finish_foreign_turns(&origin_root, "xr-sess", "claude", Some("opus")).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, foreign_root);

        // The snapshot lives in the FOREIGN repo's turn store.
        let snaps = crate::git::turns::list_turn_snapshots(&foreign_root);
        assert_eq!(snaps.len(), 1);
        let snap = &snaps[0];
        assert_eq!(snap.session_id, "xr-sess");
        assert_eq!(snap.turn_index, 0);
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].path, "handler.rs");
        assert!(snap.files[0].pre_blob.is_some());
        assert!(snap.files[0].post_blob.is_some());
        assert_ne!(snap.files[0].pre_blob, snap.files[0].post_blob);
        assert_eq!(snap.model.as_deref(), Some("opus"));
        // Foreign snapshot points back at the session's origin.
        assert_eq!(snap.cross_repo, vec![origin_root.clone()]);
        // Nothing was written to the origin's turn store by this call.
        assert!(crate::git::turns::list_turn_snapshots(&origin_root).is_empty());

        // Per-turn capture state is reset; cumulative state survives.
        let state = read_session(&origin_root, "xr-sess").unwrap();
        let capture = &state.foreign_repos.as_ref().unwrap()[&foreign_root];
        assert!(capture.turn_files.is_none());
        assert!(capture.file_edit_chain.is_none());
        assert_eq!(capture.turn_index, 1);
        assert_eq!(
            capture.last_turn_snapshot_id.as_deref(),
            Some(snap.id.as_str())
        );
        assert!(capture
            .edited_files
            .as_ref()
            .unwrap()
            .contains("handler.rs"));

        // Turn 2 chains to turn 1.
        let _ = mutate(&origin_root, "xr-sess", |state| {
            let capture = state.foreign_capture_mut(&foreign_root);
            capture.turn_started_at = Some(chrono::Utc::now().timestamp());
            capture
                .turn_files
                .get_or_insert_with(Default::default)
                .insert("handler.rs".into());
        });
        let written2 =
            finish_foreign_turns(&origin_root, "xr-sess", "claude", Some("opus")).unwrap();
        assert_eq!(written2.len(), 1);
        let snaps = crate::git::turns::list_turn_snapshots(&foreign_root);
        assert_eq!(snaps.len(), 2);
        let second = snaps.iter().find(|s| s.turn_index == 1).unwrap();
        assert_eq!(second.parent_id.as_deref(), Some(snap.id.as_str()));
    }

    #[test]
    #[serial]
    fn test_finish_foreign_turns_without_turn_activity_is_noop() {
        let _env = setup();
        let (origin, foreign) = two_repos();
        let origin_root = canon(origin.path());
        let foreign_root = canon(foreign.path());

        write_session(&origin_root, "idle-sess", "claude", None).unwrap();
        // Cumulative edits exist but nothing happened THIS turn.
        let _ = mutate(&origin_root, "idle-sess", |state| {
            let capture = state.foreign_capture_mut(&foreign_root);
            capture
                .edited_files
                .get_or_insert_with(Default::default)
                .insert("old.rs".into());
        });

        let written = finish_foreign_turns(&origin_root, "idle-sess", "claude", None).unwrap();
        assert!(written.is_empty());
        assert!(crate::git::turns::list_turn_snapshots(&foreign_root).is_empty());
    }

    #[test]
    #[serial]
    fn test_capture_gap_flagged_when_file_drifts_after_last_captured_edit() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        std::fs::write(root.join("drift.rs"), "v0\n").unwrap();
        write_session(root_str, "gap-sess", "claude", None).unwrap();
        start_turn(root_str, "gap-sess").unwrap();

        // Captured AI edit: v0 → v1.
        snapshot_pre_edit_file(root_str, "gap-sess", "drift.rs").unwrap();
        std::fs::write(root.join("drift.rs"), "v1\n").unwrap();
        record_post_edit_file(root_str, "gap-sess", "drift.rs", Some("Edit")).unwrap();
        record_edited_file(root_str, "gap-sess", "drift.rs").unwrap();

        // Uncaptured edit AFTER the last hook (human typing, formatter,
        // another session)  --  the file drifts to v2 with no hook firing.
        std::fs::write(root.join("drift.rs"), "v2\n").unwrap();

        let turn_id = finish_turn(root_str, "gap-sess", "claude", None, None)
            .unwrap()
            .unwrap();
        let snap = crate::git::turns::read_turn_snapshot(root_str, &turn_id).unwrap();
        let file = snap.files.iter().find(|f| f.path == "drift.rs").unwrap();
        assert!(
            file.capture_gap,
            "drift after the last captured edit must be flagged as a gap"
        );
    }

    #[test]
    #[serial]
    fn test_no_capture_gap_when_chain_explains_content() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        std::fs::write(root.join("clean.rs"), "v0\n").unwrap();
        write_session(root_str, "clean-sess", "claude", None).unwrap();
        start_turn(root_str, "clean-sess").unwrap();

        for v in ["v1", "v2"] {
            snapshot_pre_edit_file(root_str, "clean-sess", "clean.rs").unwrap();
            std::fs::write(root.join("clean.rs"), format!("{v}\n")).unwrap();
            record_post_edit_file(root_str, "clean-sess", "clean.rs", Some("Edit")).unwrap();
        }
        record_edited_file(root_str, "clean-sess", "clean.rs").unwrap();

        let turn_id = finish_turn(root_str, "clean-sess", "claude", None, None)
            .unwrap()
            .unwrap();
        let snap = crate::git::turns::read_turn_snapshot(root_str, &turn_id).unwrap();
        let file = snap.files.iter().find(|f| f.path == "clean.rs").unwrap();
        assert!(
            !file.capture_gap,
            "fully captured chain must not be flagged"
        );
    }

    #[test]
    #[serial]
    fn test_capture_gap_flagged_on_interior_chain_discontinuity() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        std::fs::write(root.join("gap.rs"), "v0\n").unwrap();
        write_session(root_str, "mid-sess", "claude", None).unwrap();
        start_turn(root_str, "mid-sess").unwrap();

        // Captured edit 1: v0 → v1.
        snapshot_pre_edit_file(root_str, "mid-sess", "gap.rs").unwrap();
        std::fs::write(root.join("gap.rs"), "v1\n").unwrap();
        record_post_edit_file(root_str, "mid-sess", "gap.rs", Some("Edit")).unwrap();

        // Uncaptured edit slips in: v1 → vX (no pre-tool-use hook).
        std::fs::write(root.join("gap.rs"), "vX\n").unwrap();

        // Captured edit 2 starts from vX: chain pre won't match prior post.
        snapshot_pre_edit_file(root_str, "mid-sess", "gap.rs").unwrap();
        std::fs::write(root.join("gap.rs"), "v2\n").unwrap();
        record_post_edit_file(root_str, "mid-sess", "gap.rs", Some("Edit")).unwrap();
        record_edited_file(root_str, "mid-sess", "gap.rs").unwrap();

        let turn_id = finish_turn(root_str, "mid-sess", "claude", None, None)
            .unwrap()
            .unwrap();
        let snap = crate::git::turns::read_turn_snapshot(root_str, &turn_id).unwrap();
        let file = snap.files.iter().find(|f| f.path == "gap.rs").unwrap();
        assert!(
            file.capture_gap,
            "interior discontinuity (uncaptured edit between captured pairs) must be flagged"
        );
    }

    #[test]
    #[serial]
    fn test_event_seq_is_monotonic_across_origin_and_foreign_edits() {
        let _env = setup();
        let (origin, foreign) = two_repos();
        let origin_root = canon(origin.path());
        let foreign_root = canon(foreign.path());

        std::fs::write(origin.path().join("local.rs"), "l0\n").unwrap();
        std::fs::write(foreign.path().join("remote.rs"), "r0\n").unwrap();
        write_session(&origin_root, "seq-sess", "claude", None).unwrap();

        // Edit 1: origin file.
        snapshot_pre_edit_file(&origin_root, "seq-sess", "local.rs").unwrap();
        std::fs::write(origin.path().join("local.rs"), "l1\n").unwrap();
        record_post_edit_file(&origin_root, "seq-sess", "local.rs", Some("Edit")).unwrap();

        // Edit 2: foreign file.
        snapshot_pre_edit_file_in_repo(&origin_root, "seq-sess", &foreign_root, "remote.rs")
            .unwrap();
        std::fs::write(foreign.path().join("remote.rs"), "r1\n").unwrap();
        record_post_edit_file_in_repo(
            &origin_root,
            "seq-sess",
            &foreign_root,
            "remote.rs",
            Some("Edit"),
        )
        .unwrap();

        // Edit 3: origin file again.
        snapshot_pre_edit_file(&origin_root, "seq-sess", "local.rs").unwrap();
        std::fs::write(origin.path().join("local.rs"), "l2\n").unwrap();
        record_post_edit_file(&origin_root, "seq-sess", "local.rs", Some("Edit")).unwrap();

        let state = read_session(&origin_root, "seq-sess").unwrap();
        let origin_pairs = &state.file_edit_chain.as_ref().unwrap()["local.rs"];
        let foreign_pairs = &state.foreign_repos.as_ref().unwrap()[&foreign_root]
            .file_edit_chain
            .as_ref()
            .unwrap()["remote.rs"];

        assert_eq!(origin_pairs[0].seq, 1);
        assert_eq!(foreign_pairs[0].seq, 2);
        assert_eq!(origin_pairs[1].seq, 3);
        assert_eq!(state.event_seq, 3);
        assert!(origin_pairs[0].timestamp_us > 0);
        assert!(foreign_pairs[0].timestamp_us >= origin_pairs[0].timestamp_us);
    }

    #[test]
    #[serial]
    fn test_origin_finish_turn_links_cross_repo() {
        let _env = setup();
        let (origin, foreign) = two_repos();
        let origin_root = canon(origin.path());
        let foreign_root = canon(foreign.path());

        write_session(&origin_root, "link-sess", "claude", None).unwrap();
        start_turn(&origin_root, "link-sess").unwrap();
        std::fs::write(origin.path().join("local.rs"), "x\n").unwrap();
        record_edited_file(&origin_root, "link-sess", "local.rs").unwrap();
        snapshot_session_files(&origin_root, "link-sess", &["local.rs".to_string()]).unwrap();
        let _ = mutate(&origin_root, "link-sess", |state| {
            let capture = state.foreign_capture_mut(&foreign_root);
            capture
                .turn_files
                .get_or_insert_with(Default::default)
                .insert("remote.rs".into());
        });

        let turn_id = finish_turn(&origin_root, "link-sess", "claude", None, None)
            .unwrap()
            .unwrap();
        let snap = crate::git::turns::read_turn_snapshot(&origin_root, &turn_id).unwrap();
        assert_eq!(
            snap.cross_repo,
            vec![foreign_root.clone()],
            "origin turn snapshot must link to the foreign repo it edited"
        );
    }

    /// End-to-end through the real hook entrypoint: a session whose cwd is
    /// repo X edits a file in repo Y via pre-tool-use/after-tool-use, then
    /// stops. Repo Y must end up with a self-contained turn snapshot.
    #[test]
    #[serial]
    fn test_handle_event_routes_cross_repo_edit() {
        let _env = setup();
        let (origin, foreign) = two_repos();
        let origin_root = canon(origin.path());
        let foreign_root = canon(foreign.path());

        let foreign_file = std::path::Path::new(&foreign_root).join("src").join("y.rs");
        std::fs::create_dir_all(foreign_file.parent().unwrap()).unwrap();
        std::fs::write(&foreign_file, "before\n").unwrap();
        let foreign_file = foreign_file.to_string_lossy().to_string();

        let ev = |name: &str, payload: serde_json::Value| {
            crate::hooks::handle_event(name, &payload.to_string(), Some("claude")).unwrap();
        };

        ev(
            "session-start",
            serde_json::json!({"session_id": "e2e-sess", "cwd": origin_root}),
        );
        ev(
            "pre-tool-use",
            serde_json::json!({
                "session_id": "e2e-sess",
                "cwd": origin_root,
                "tool_name": "Edit",
                "tool_input": {"file_path": foreign_file},
            }),
        );
        std::fs::write(&foreign_file, "after\n").unwrap();
        ev(
            "after-tool-use",
            serde_json::json!({
                "session_id": "e2e-sess",
                "cwd": origin_root,
                "tool_name": "Edit",
                "tool_input": {"file_path": foreign_file},
            }),
        );
        ev(
            "stop",
            serde_json::json!({"session_id": "e2e-sess", "cwd": origin_root}),
        );

        let snaps = crate::git::turns::list_turn_snapshots(&foreign_root);
        assert_eq!(
            snaps.len(),
            1,
            "repo Y must receive a turn snapshot for the cross-repo edit"
        );
        let snap = &snaps[0];
        assert_eq!(snap.session_id, "e2e-sess");
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].path, "src/y.rs");
        assert!(snap.files[0].pre_blob.is_some());
        assert!(snap.files[0].post_blob.is_some());
        assert_ne!(snap.files[0].pre_blob, snap.files[0].post_blob);

        // The session is discoverable from repo Y's side.
        let sessions = active_sessions(&foreign_root);
        assert!(
            sessions.iter().any(|s| s.session_id == "e2e-sess"),
            "session must be listed for the foreign repo it edited"
        );
    }

    #[test]
    #[serial]
    fn test_ended_session_not_re_ended() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "dupe-end", "claude", None).unwrap();
        remove_session(root_str, "dupe-end");
        let first_ended = read_session(root_str, "dupe-end")
            .unwrap()
            .ended_at
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));
        remove_session(root_str, "dupe-end");
        let second_ended = read_session(root_str, "dupe-end")
            .unwrap()
            .ended_at
            .unwrap();

        // Both calls should set ended_at; second call just updates the timestamp.
        assert!(second_ended >= first_ended);
    }

    // ── E1 fix: verify SessionBatch holds lock across batch mutations ───

    #[test]
    #[serial]
    fn test_session_batch_holds_lock_during_mutations() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "batch-lock", "cursor", Some("opus")).unwrap();
        let mut batch = SessionBatch::open(root_str, "batch-lock", "cursor", None).unwrap();

        // While batch is open, direct read_modify_write should fail (lock held).
        let result = store::read_modify_write(root_str, "batch-lock", |s| {
            s.current_turn_index = 999;
        });
        assert!(
            result.is_err(),
            "read_modify_write must fail while SessionBatch holds the lock"
        );

        // But we can still mutate through the batch.
        batch.state_mut().current_turn_index = 42;
        batch.flush().unwrap();

        // After flush (which drops the lock), state is persisted.
        let state = read_session(root_str, "batch-lock").unwrap();
        assert_eq!(state.current_turn_index, 42);
    }

    // ── E1 fix: write_session resume path uses atomic RMW ───────────────

    #[test]
    #[serial]
    fn test_write_session_resume_clears_ended_atomically() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "resume-sess", "cursor", Some("claude")).unwrap();
        remove_session(root_str, "resume-sess");

        let ended = read_session(root_str, "resume-sess").unwrap();
        assert!(ended.ended_at.is_some(), "session should be ended");

        // Resume the session with a new model.
        write_session(root_str, "resume-sess", "cursor", Some("opus")).unwrap();
        let resumed = read_session(root_str, "resume-sess").unwrap();
        assert!(resumed.ended_at.is_none(), "ended_at should be cleared");
        assert_eq!(
            resumed.model.as_deref(),
            Some("opus"),
            "model should be updated"
        );
    }

    // ── E1 fix: mutate uses atomic RMW ──────────────────────────────────

    #[test]
    #[serial]
    fn test_mutate_persists_changes_atomically() {
        let _env = setup();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "atomic-mut", "cursor", None).unwrap();

        let initial_ts = read_session(root_str, "atomic-mut").unwrap().updated_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        mutate(root_str, "atomic-mut", |s| {
            s.current_turn_index = 10;
        })
        .unwrap();
        let after = read_session(root_str, "atomic-mut").unwrap();
        assert_eq!(after.current_turn_index, 10);
        assert!(
            after.updated_at >= initial_ts,
            "bump() should update updated_at"
        );
    }
}
