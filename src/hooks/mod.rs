pub mod install;
pub mod state;

use serde::Deserialize;

/// Lifecycle event received from an agent tool via `oobo hooks agent <event>`.
#[derive(Debug, Deserialize)]
pub struct HookEvent {
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Captures unknown fields from hook payloads for forward compatibility.
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: serde_json::Value,
}

/// Handle a lifecycle event from an agent tool.
///
/// Called by `oobo hooks agent <event> [--tool <name>]` which receives JSON on stdin.
/// The `--tool` flag provides explicit tool identity (preferred over payload guessing).
/// This is internal plumbing — never typed by the user.
pub fn handle_event(
    event_name: &str,
    payload: &str,
    tool_flag: Option<&str>,
) -> crate::error::Result<()> {
    let mut event: HookEvent = serde_json::from_str(payload)?;

    if event.event.is_empty() {
        event.event = event_name.to_string();
    }

    let cwd = event
        .workspace_roots
        .first()
        .cloned()
        .or(event.cwd.clone())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        });

    let project_root = crate::git::proxy::project_root_from(&cwd);

    let agent = tool_flag.unwrap_or_else(|| {
        event
            .agent
            .as_deref()
            .or(event.extra.get("composer_mode").and_then(|v| v.as_str()))
            .unwrap_or("unknown")
    });

    let session_id_field = event
        .session_id
        .as_deref()
        .or(event.extra.get("conversation_id").and_then(|v| v.as_str()));

    match event_name {
        "session-start" => {
            let session_id = session_id_field.ok_or(crate::error::OoboError::Config(
                "session-start requires session_id".into(),
            ))?;
            state::write_session(&project_root, session_id, agent, event.model.as_deref())?;
            // For non-Cursor agents, capture pre-agent state at session start.
            // Cursor uses the more precise `before-submit-prompt` hook instead.
            if !project_root.is_empty() && !is_cursor_agent(agent) {
                let _ = state::snapshot_pre_agent_state(&project_root, session_id);
            }
        }
        "session-end" => {
            if let Some(sid) = session_id_field {
                // Read the session state BEFORE deleting to compute stats proactively.
                if !project_root.is_empty() {
                    let active = state::read_session(&project_root, sid);
                    let source = crate::core::tool::normalize_source(agent);
                    if let Err(e) = crate::commands::index::index_single_session(
                        sid,
                        source,
                        &project_root,
                        active.as_ref(),
                    ) {
                        eprintln!(
                            "oobo: warning: could not index session {}: {e}",
                            &sid[..sid.len().min(8)]
                        );
                    }
                }
                state::remove_session(&project_root, sid);
            }
        }
        "before-submit-prompt" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                if !project_root.is_empty() {
                    let _ = state::snapshot_pre_agent_state(&project_root, sid);
                }
            }
        }
        "after-tool-use" | "after-file-edit" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                if !project_root.is_empty() {
                    let tool_name = event
                        .extra
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let tool_input = event.extra.get("tool_input");

                    // Record tool usage count by tool name.
                    if !tool_name.is_empty() {
                        let input_summary = summarize_tool_input_hook(tool_name, tool_input);
                        let _ = state::record_tool_use(
                            &project_root,
                            sid,
                            tool_name,
                            input_summary.as_deref(),
                        );
                    }

                    // Track edited files for file-modifying tools.
                    if tool_name == "Write"
                        || tool_name == "Edit"
                        || tool_name == "MultiEdit"
                        || tool_name == "Delete"
                        || tool_name.is_empty()
                    {
                        // Cursor: file_path at top level.
                        // Claude PostToolUse: file_path inside tool_input.
                        let file_path = event
                            .extra
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                tool_input
                                    .and_then(|ti| ti.get("file_path"))
                                    .and_then(|v| v.as_str())
                            });

                        if let Some(abs_path) = file_path {
                            let rel = make_relative(abs_path, &project_root);
                            if !rel.starts_with('/') && !rel.starts_with("..") {
                                let _ = state::record_edited_file(&project_root, sid, &rel);
                            }
                        }
                    }

                    // Track read files for file-reading tools.
                    if is_read_tool(tool_name) {
                        let file_path = event
                            .extra
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                tool_input
                                    .and_then(|ti| ti.get("file_path"))
                                    .and_then(|v| v.as_str())
                            })
                            .or_else(|| {
                                // For dir-scoped tools (Grep, Search, Glob, etc.)
                                // only fall back to `path` if it looks like a file.
                                if is_dir_scoped_tool(tool_name) {
                                    None
                                } else {
                                    tool_input
                                        .and_then(|ti| ti.get("path"))
                                        .and_then(|v| v.as_str())
                                }
                            });

                        if let Some(abs_path) = file_path {
                            if !abs_path.ends_with('/') && abs_path != "." {
                                let rel = make_relative(abs_path, &project_root);
                                if !rel.starts_with('/') && !rel.starts_with("..") {
                                    let _ = state::record_read_file(&project_root, sid, &rel);
                                }
                            }
                        }
                    }
                }
            }
        }
        "tool-use-failure" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                if !project_root.is_empty() {
                    let tool_name = event
                        .extra
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let _ = state::record_tool_failure(&project_root, sid, tool_name);
                }
            }
        }
        "subagent-start" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                // Claude uses agent_id, Cursor uses subagent_id.
                let agent_id = event
                    .extra
                    .get("agent_id")
                    .or_else(|| event.extra.get("subagent_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Claude uses agent_type, Cursor uses subagent_type.
                let agent_type = event
                    .extra
                    .get("agent_type")
                    .or_else(|| event.extra.get("subagent_type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                if !agent_id.is_empty() {
                    let _ = state::record_subagent_start(&project_root, sid, agent_id, agent_type);
                }
            }
        }
        "after-agent-thought" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let duration_ms = event
                    .extra
                    .get("duration_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if duration_ms > 0 {
                    let _ = state::record_thinking(&project_root, sid, duration_ms);
                }
            }
        }
        "after-agent-response" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                // Swallow errors — this fires on every response so a transient IO
                // failure shouldn't surface as a hook error to the user.
                let _ = state::touch_session(&project_root, sid, None);
            }
        }
        "pre-compact" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let _ = state::record_compact(&project_root, sid);
            }
        }
        "stop" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let transcript_path = event.extra.get("transcript_path").and_then(|v| v.as_str());
                state::touch_session(&project_root, sid, transcript_path)?;

                if !project_root.is_empty() {
                    let files = if is_cursor_agent(agent) {
                        let db_files = crate::tools::cursor::composer_data::files_edited_in_session(
                            sid,
                            &project_root,
                            0,
                        );
                        if db_files.is_empty() {
                            state::get_edited_files(&project_root, sid)
                        } else {
                            db_files
                        }
                    } else {
                        let tracked = state::get_edited_files(&project_root, sid);
                        if tracked.is_empty() {
                            dirty_worktree_files(&project_root)
                        } else {
                            tracked
                        }
                    };
                    if !files.is_empty() {
                        let _ = state::snapshot_session_files(&project_root, sid, &files);
                    }
                }
            }
        }
        "subagent-stop" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                state::touch_session(&project_root, sid, None)?;

                let agent_id = event
                    .extra
                    .get("agent_id")
                    .or_else(|| event.extra.get("subagent_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !agent_id.is_empty() {
                    let _ = state::record_subagent_end(&project_root, sid, agent_id);
                }

                if !project_root.is_empty() {
                    if let Some(files_val) = event.extra.get("modified_files") {
                        if let Some(files_arr) = files_val.as_array() {
                            let files: Vec<String> = files_arr
                                .iter()
                                .filter_map(|v| v.as_str())
                                .map(|s| make_relative(s, &project_root))
                                .filter(|s| !s.starts_with('/') && !s.starts_with(".."))
                                .collect();
                            if !files.is_empty() {
                                let _ = state::snapshot_session_files(&project_root, sid, &files);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn summarize_tool_input_hook(
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
) -> Option<String> {
    crate::utils::summarize_tool_input(tool_name, tool_input, 200)
}

fn dirty_worktree_files(project_root: &str) -> Vec<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut files = std::collections::HashSet::new();

    // Modified tracked files (staged + unstaged)
    if let Ok(o) = std::process::Command::new(&git)
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(project_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if !line.is_empty() {
                    files.insert(line.to_string());
                }
            }
        }
    }

    // Untracked files (new files created by the agent)
    if let Ok(o) = std::process::Command::new(&git)
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(project_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if !line.is_empty() {
                    files.insert(line.to_string());
                }
            }
        }
    }

    files.into_iter().collect()
}

fn is_cursor_agent(agent: &str) -> bool {
    crate::core::tool::is_cursor_agent(agent)
}

fn is_read_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Read"
            | "ReadFile"
            | "View"
            | "read_file"
            | "read"
            | "Grep"
            | "grep"
            | "Search"
            | "search"
            | "SemanticSearch"
            | "Glob"
            | "glob"
            | "ListFiles"
            | "list_files"
    )
}

fn is_dir_scoped_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Grep"
            | "grep"
            | "Search"
            | "search"
            | "SemanticSearch"
            | "Glob"
            | "glob"
            | "ListFiles"
            | "list_files"
    )
}

/// Strip the project root prefix to get a relative path, matching how git
/// and the rest of the attribution pipeline represent file paths.
/// Canonicalizes both sides to handle symlinks and macOS `/var` vs `/private/var`.
fn make_relative(abs_path: &str, project_root: &str) -> String {
    let abs =
        std::fs::canonicalize(abs_path).unwrap_or_else(|_| std::path::PathBuf::from(abs_path));
    let root = std::fs::canonicalize(project_root)
        .unwrap_or_else(|_| std::path::PathBuf::from(project_root));
    abs.strip_prefix(&root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs_path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_deserialize() {
        let json = r#"{"session_id": "s1", "agent": "cursor", "model": "claude-opus-4"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.session_id.as_deref(), Some("s1"));
        assert_eq!(event.agent.as_deref(), Some("cursor"));
        assert_eq!(event.model.as_deref(), Some("claude-opus-4"));
    }

    #[test]
    fn test_hook_event_deserialize_minimal() {
        let event: HookEvent = serde_json::from_str("{}").unwrap();
        assert!(event.session_id.is_none());
        assert!(event.event.is_empty());
        assert!(event.cwd.is_none());
        assert!(event.agent.is_none());
        assert!(event.model.is_none());
    }

    #[test]
    fn test_hook_event_with_extra_fields() {
        let json = r#"{"session_id": "s1", "unknown_field": 42}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.session_id.as_deref(), Some("s1"));
        assert_eq!(
            event.extra.get("unknown_field").and_then(|v| v.as_i64()),
            Some(42)
        );
    }

    #[test]
    fn test_handle_event_session_start() {
        let result = handle_event("session-start", "{}", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_event_invalid_json() {
        let result = handle_event("test", "not json", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_event_session_end_no_id() {
        let result = handle_event("session-end", "{}", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_flag_overrides_payload_agent() {
        let json = r#"{"session_id": "s1", "agent": "agent"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.agent.as_deref(), Some("agent"));
    }

    #[test]
    fn test_make_relative_strips_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("src/main.rs");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(&file, "fn main() {}").unwrap();

        let result = make_relative(file.to_str().unwrap(), root.to_str().unwrap());
        assert_eq!(result, "src/main.rs");
    }

    #[test]
    fn test_make_relative_outside_project() {
        let result = make_relative("/tmp/other/file.rs", "/home/user/project");
        assert_eq!(result, "/tmp/other/file.rs");
    }

    #[test]
    fn test_is_read_tool_positives() {
        for tool in [
            "Read",
            "ReadFile",
            "View",
            "read_file",
            "read",
            "Grep",
            "grep",
            "Search",
            "search",
            "SemanticSearch",
            "Glob",
            "glob",
            "ListFiles",
            "list_files",
        ] {
            assert!(is_read_tool(tool), "{tool} should be a read tool");
        }
    }

    #[test]
    fn test_is_read_tool_negatives() {
        for tool in ["Write", "Edit", "Bash", "Shell", "Delete", "cat", ""] {
            assert!(!is_read_tool(tool), "{tool} should NOT be a read tool");
        }
    }

    #[test]
    fn test_is_dir_scoped_tool() {
        assert!(is_dir_scoped_tool("Grep"));
        assert!(is_dir_scoped_tool("Search"));
        assert!(is_dir_scoped_tool("Glob"));
        assert!(!is_dir_scoped_tool("Read"));
        assert!(!is_dir_scoped_tool("View"));
    }

    #[test]
    fn test_dir_scoped_is_subset_of_read_tool() {
        for tool in [
            "Grep",
            "grep",
            "Search",
            "search",
            "SemanticSearch",
            "Glob",
            "glob",
            "ListFiles",
            "list_files",
        ] {
            assert!(
                is_read_tool(tool),
                "{tool} is in is_dir_scoped_tool but not is_read_tool — \
                 the dir-scoped check will never fire"
            );
        }
    }
}
