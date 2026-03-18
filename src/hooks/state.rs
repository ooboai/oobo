use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Atomically write a JSON file by writing to a temp file then renaming.
/// This prevents partial writes from corrupting session state when
/// multiple hooks fire concurrently (e.g. stop + subagent-stop).
fn atomic_write_json(path: &Path, json: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(json.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// A subagent spawned during an agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentRun {
    pub agent_id: String,
    pub agent_type: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

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
    /// Git blob hashes of files BEFORE the agent's edit (pre-agent state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_agent_snapshots: Option<std::collections::HashMap<String, String>>,
    /// Git blob hashes of files AFTER the agent's last edit (post-agent state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_snapshots: Option<std::collections::HashMap<String, String>>,
    /// Files edited by the agent, accumulated from PostToolUse hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_files: Option<std::collections::HashSet<String>>,
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

fn git_common_dir(project_root: &str) -> PathBuf {
    crate::git::detect::resolve_git_common_dir(project_root)
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
        pre_agent_snapshots: None,
        file_snapshots: None,
        edited_files: None,
        tool_usage: None,
        tool_failures: None,
        bash_commands: None,
        subagent_runs: None,
        thinking_duration_ms: None,
        compact_count: None,
        started_at: now,
        updated_at: now,
    };

    let json = serde_json::to_string_pretty(&state)?;
    let path = session_path(project_root, session_id);
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Ensure a session state file exists. If `sessionStart` fired before
/// `git init`, the file won't exist yet. This creates it so the session
/// is tracked for subsequent commits.
pub fn ensure_session(
    project_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
) -> Result<()> {
    if project_root.is_empty() {
        log_ensure("skip: empty project_root", session_id);
        return Ok(());
    }
    let path = session_path(project_root, session_id);
    if path.exists() {
        return Ok(());
    }
    let dir = sessions_dir(project_root);
    if !dir.parent().map(|p| p.exists()).unwrap_or(false) {
        log_ensure(
            &format!("skip: parent dir missing for {}", dir.display()),
            session_id,
        );
        return Ok(());
    }
    log_ensure(&format!("creating in {}", dir.display()), session_id);
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
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Record a file edited by the agent during this session.
/// Called from `after-tool-use` hook events for file-modifying tools (Write,
/// Edit, MultiEdit, Delete). Accumulates file paths so the `stop` handler can
/// snapshot exactly the agent-touched files instead of scanning the entire worktree.
pub fn record_edited_file(project_root: &str, session_id: &str, file_path: &str) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let mut files = state.edited_files.unwrap_or_default();
    files.insert(file_path.to_string());
    state.edited_files = Some(files);
    state.updated_at = chrono::Utc::now().timestamp();

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Record a tool call by the agent. Increments the tool_usage count and
/// optionally stores a bash command summary.
pub fn record_tool_use(
    project_root: &str,
    session_id: &str,
    tool_name: &str,
    input_summary: Option<&str>,
) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let mut usage = state.tool_usage.unwrap_or_default();
    *usage.entry(tool_name.to_string()).or_insert(0) += 1;
    state.tool_usage = Some(usage);

    if tool_name == "Bash" || tool_name == "Shell" {
        if let Some(cmd) = input_summary {
            let mut cmds = state.bash_commands.unwrap_or_default();
            const MAX_BASH_COMMANDS: usize = 50;
            if cmds.len() >= MAX_BASH_COMMANDS {
                cmds.remove(0);
            }
            cmds.push(cmd.to_string());
            state.bash_commands = Some(cmds);
        }
    }

    state.updated_at = chrono::Utc::now().timestamp();

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Record a failed tool call.
pub fn record_tool_failure(project_root: &str, session_id: &str, tool_name: &str) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let failures = state.tool_failures.unwrap_or(0);
    state.tool_failures = Some(failures + 1);

    // PostToolUseFailure fires *instead of* PostToolUse (not in addition),
    // so we count failures in tool_usage to keep the total accurate.
    let mut usage = state.tool_usage.unwrap_or_default();
    *usage.entry(tool_name.to_string()).or_insert(0) += 1;
    state.tool_usage = Some(usage);

    state.updated_at = chrono::Utc::now().timestamp();

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Record a subagent spawn.
pub fn record_subagent_start(
    project_root: &str,
    session_id: &str,
    agent_id: &str,
    agent_type: &str,
) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let mut runs = state.subagent_runs.unwrap_or_default();
    runs.push(SubagentRun {
        agent_id: agent_id.to_string(),
        agent_type: agent_type.to_string(),
        started_at: chrono::Utc::now().timestamp(),
        ended_at: None,
    });
    state.subagent_runs = Some(runs);
    state.updated_at = chrono::Utc::now().timestamp();

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Record a subagent completing by setting its ended_at timestamp.
pub fn record_subagent_end(project_root: &str, session_id: &str, agent_id: &str) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    if let Some(ref mut runs) = state.subagent_runs {
        for run in runs.iter_mut().rev() {
            if run.agent_id == agent_id && run.ended_at.is_none() {
                run.ended_at = Some(chrono::Utc::now().timestamp());
                break;
            }
        }
    }
    state.updated_at = chrono::Utc::now().timestamp();

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Record thinking duration from an afterAgentThought hook.
pub fn record_thinking(project_root: &str, session_id: &str, duration_ms: u64) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let current = state.thinking_duration_ms.unwrap_or(0);
    state.thinking_duration_ms = Some(current + duration_ms);
    state.updated_at = chrono::Utc::now().timestamp();

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Record a context compaction event.
pub fn record_compact(project_root: &str, session_id: &str) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let current = state.compact_count.unwrap_or(0);
    state.compact_count = Some(current + 1);
    state.updated_at = chrono::Utc::now().timestamp();

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Read the accumulated edited_files set from a session's state.
pub fn get_edited_files(project_root: &str, session_id: &str) -> Vec<String> {
    let path = session_path(project_root, session_id);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let state: ActiveSession = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    state
        .edited_files
        .map(|s| s.into_iter().collect())
        .unwrap_or_default()
}

/// Read and deserialize the active session state file, if it exists.
pub fn read_session(project_root: &str, session_id: &str) -> Option<ActiveSession> {
    let path = session_path(project_root, session_id);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Read the model field from a session state file.
/// Deserializes the full `ActiveSession`; consider caching if called in a loop.
pub fn read_session_model(project_root: &str, session_id: &str) -> Option<String> {
    let state = read_session(project_root, session_id)?;
    state.model
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

/// List active sessions filtered to only those belonging to the given worktree.
/// Sessions without a worktree field (pre-upgrade) are included in all worktrees
/// for backward compatibility.
pub fn active_sessions_for_worktree(project_root: &str) -> Vec<ActiveSession> {
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

/// Capture the pre-agent file state: snapshot currently dirty files in the
/// worktree. Called on `before-submit-prompt` — any worktree changes at this
/// moment are human edits (the agent hasn't started yet).
pub fn snapshot_pre_agent_state(project_root: &str, session_id: &str) -> Result<()> {
    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());

    // Get all modified/new files in the worktree (tracked + untracked)
    let mut dirty_files: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Ok(o) = Command::new(&git)
        .args(["diff", "--name-only", "HEAD"])
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

    if let Ok(o) = Command::new(&git)
        .args(["ls-files", "--others", "--exclude-standard"])
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

    if dirty_files.is_empty() {
        // No dirty files → HEAD IS the pre-agent state. No snapshot needed;
        // at commit time we'll use HEAD~1 as the baseline.
        return Ok(());
    }

    let mut snapshots = std::collections::HashMap::new();

    for file in &dirty_files {
        let abs_path = std::path::Path::new(project_root).join(file);
        if !abs_path.exists() {
            continue;
        }
        let output = Command::new(&git)
            .args(["hash-object", "-w"])
            .arg(&abs_path)
            .current_dir(project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();

        if let Ok(o) = output {
            if o.status.success() {
                let hash = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !hash.is_empty() {
                    snapshots.insert(file.clone(), hash);
                }
            }
        }
    }

    state.pre_agent_snapshots = if snapshots.is_empty() {
        None
    } else {
        Some(snapshots)
    };

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
}

/// Snapshot files edited by this session into git's object store.
/// For each file, runs `git hash-object -w <file>` and stores the blob hash
/// in the session's `file_snapshots` map. This captures the exact file state
/// after the agent's edit, before any human modifications.
pub fn snapshot_session_files(
    project_root: &str,
    session_id: &str,
    files: &[String],
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let path = session_path(project_root, session_id);
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut state: ActiveSession = serde_json::from_str(&content)?;

    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut snapshots = state.file_snapshots.unwrap_or_default();

    for file in files {
        let abs_path = Path::new(project_root).join(file);
        if !abs_path.exists() {
            continue;
        }
        let output = Command::new(&git)
            .args(["hash-object", "-w"])
            .arg(&abs_path)
            .current_dir(project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();

        if let Ok(o) = output {
            if o.status.success() {
                let hash = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !hash.is_empty() {
                    snapshots.insert(file.clone(), hash);
                }
            }
        }
    }

    state.file_snapshots = if snapshots.is_empty() {
        None
    } else {
        Some(snapshots)
    };

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write_json(&path, &json)?;

    Ok(())
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
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: None,
            tool_usage: None,
            tool_failures: None,
            bash_commands: None,
            subagent_runs: None,
            thinking_duration_ms: None,
            compact_count: None,
            started_at: now,
            updated_at: now,
        };
        let other_wt = ActiveSession {
            session_id: "s2".into(),
            agent: "claude".into(),
            model: None,
            worktree: Some("/other/worktree".into()),
            transcript_path: None,
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: None,
            tool_usage: None,
            tool_failures: None,
            bash_commands: None,
            subagent_runs: None,
            thinking_duration_ms: None,
            compact_count: None,
            started_at: now,
            updated_at: now,
        };
        let no_wt = ActiveSession {
            session_id: "s3".into(),
            agent: "gemini".into(),
            model: None,
            worktree: None,
            transcript_path: None,
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: None,
            tool_usage: None,
            tool_failures: None,
            bash_commands: None,
            subagent_runs: None,
            thinking_duration_ms: None,
            compact_count: None,
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

    #[test]
    fn test_record_and_get_edited_files() {
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
    fn test_record_edited_file_nonexistent_session() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        let result = record_edited_file(root_str, "no-such-session", "file.rs");
        assert!(result.is_ok()); // no-op, not an error
        assert!(get_edited_files(root_str, "no-such-session").is_empty());
    }

    #[test]
    fn test_record_tool_use_and_bash_commands() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "tool-sess", "cursor", None).unwrap();

        record_tool_use(root_str, "tool-sess", "Read", Some("/src/main.rs")).unwrap();
        record_tool_use(root_str, "tool-sess", "Read", Some("/src/lib.rs")).unwrap();
        record_tool_use(root_str, "tool-sess", "Bash", Some("ls -la")).unwrap();
        record_tool_use(root_str, "tool-sess", "Shell", Some("cargo build")).unwrap();

        let path = session_path(root_str, "tool-sess");
        let state: ActiveSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        let usage = state.tool_usage.unwrap();
        assert_eq!(usage.get("Read"), Some(&2));
        assert_eq!(usage.get("Bash"), Some(&1));
        assert_eq!(usage.get("Shell"), Some(&1));

        let cmds = state.bash_commands.unwrap();
        assert_eq!(cmds, vec!["ls -la", "cargo build"]);
    }

    #[test]
    fn test_bash_commands_cap_at_50() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "cap-sess", "claude", None).unwrap();

        for i in 0..60 {
            record_tool_use(root_str, "cap-sess", "Bash", Some(&format!("cmd-{i}"))).unwrap();
        }

        let path = session_path(root_str, "cap-sess");
        let state: ActiveSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        let cmds = state.bash_commands.unwrap();
        assert_eq!(cmds.len(), 50);
        assert_eq!(cmds[0], "cmd-10");
        assert_eq!(cmds[49], "cmd-59");
    }

    #[test]
    fn test_record_tool_failure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "fail-sess", "claude", None).unwrap();

        record_tool_failure(root_str, "fail-sess", "Write").unwrap();
        record_tool_failure(root_str, "fail-sess", "Write").unwrap();
        record_tool_failure(root_str, "fail-sess", "Edit").unwrap();

        let path = session_path(root_str, "fail-sess");
        let state: ActiveSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(state.tool_failures, Some(3));
        let usage = state.tool_usage.unwrap();
        assert_eq!(usage.get("Write"), Some(&2));
        assert_eq!(usage.get("Edit"), Some(&1));
    }

    #[test]
    fn test_subagent_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "sub-sess", "cursor", None).unwrap();

        record_subagent_start(root_str, "sub-sess", "agent-1", "explore").unwrap();
        record_subagent_start(root_str, "sub-sess", "agent-2", "code").unwrap();
        record_subagent_end(root_str, "sub-sess", "agent-1").unwrap();

        let path = session_path(root_str, "sub-sess");
        let state: ActiveSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        let runs = state.subagent_runs.unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].agent_id, "agent-1");
        assert!(runs[0].ended_at.is_some());
        assert_eq!(runs[1].agent_id, "agent-2");
        assert!(runs[1].ended_at.is_none());
    }

    #[test]
    fn test_record_thinking_accumulates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "think-sess", "cursor", None).unwrap();

        record_thinking(root_str, "think-sess", 1500).unwrap();
        record_thinking(root_str, "think-sess", 2500).unwrap();

        let path = session_path(root_str, "think-sess");
        let state: ActiveSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(state.thinking_duration_ms, Some(4000));
    }

    #[test]
    fn test_record_compact_increments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "compact-sess", "cursor", None).unwrap();

        record_compact(root_str, "compact-sess").unwrap();
        record_compact(root_str, "compact-sess").unwrap();
        record_compact(root_str, "compact-sess").unwrap();

        let path = session_path(root_str, "compact-sess");
        let state: ActiveSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(state.compact_count, Some(3));
    }

    #[test]
    fn test_read_session_returns_active_session() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "read-test", "claude", Some("claude-opus-4")).unwrap();

        let session = read_session(root_str, "read-test");
        assert!(session.is_some());
        let session = session.unwrap();
        assert_eq!(session.session_id, "read-test");
        assert_eq!(session.agent, "claude");
        assert_eq!(session.model.as_deref(), Some("claude-opus-4"));
    }

    #[test]
    fn test_read_session_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        assert!(read_session(root_str, "nonexistent").is_none());
    }

    #[test]
    fn test_read_session_model_extracts_model() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "model-test", "cursor", Some("gpt-4o")).unwrap();
        assert_eq!(
            read_session_model(root_str, "model-test").as_deref(),
            Some("gpt-4o")
        );
    }

    #[test]
    fn test_read_session_model_returns_none_when_no_model() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "no-model", "cursor", None).unwrap();
        assert!(read_session_model(root_str, "no-model").is_none());
    }

    #[test]
    fn test_read_session_preserves_accumulated_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_git_repo(root);
        let root_str = root.to_str().unwrap();

        write_session(root_str, "rich-sess", "claude", Some("sonnet")).unwrap();
        record_tool_use(root_str, "rich-sess", "Bash", Some("ls -la")).unwrap();
        record_tool_use(root_str, "rich-sess", "Edit", Some("main.rs")).unwrap();
        record_tool_use(root_str, "rich-sess", "Bash", Some("cargo build")).unwrap();
        record_tool_failure(root_str, "rich-sess", "Write").unwrap();
        record_thinking(root_str, "rich-sess", 500).unwrap();

        let session = read_session(root_str, "rich-sess").unwrap();
        let usage = session.tool_usage.unwrap();
        assert_eq!(usage.get("Bash"), Some(&2));
        assert_eq!(usage.get("Edit"), Some(&1));
        assert_eq!(session.tool_failures, Some(1));
        assert_eq!(session.thinking_duration_ms, Some(500));
        assert_eq!(session.bash_commands.as_ref().map(|c| c.len()), Some(2));
    }
}
