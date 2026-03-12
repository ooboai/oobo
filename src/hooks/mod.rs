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
    #[serde(default, alias = "conversation_id")]
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
                state::remove_session(&project_root, sid);
            }
        }
        "before-submit-prompt" => {
            if let Some(sid) = session_id_field {
                // Capture pre-agent state: any dirty files right now are
                // human edits (the agent hasn't started its turn yet).
                if !project_root.is_empty() {
                    let _ = state::snapshot_pre_agent_state(&project_root, sid);
                }
            }
        }
        "stop" => {
            if let Some(sid) = session_id_field {
                let transcript_path = event.extra.get("transcript_path").and_then(|v| v.as_str());
                state::touch_session(&project_root, sid, transcript_path)?;

                if !project_root.is_empty() {
                    if is_cursor_agent(agent) {
                        // Cursor: use composerData for precise file list.
                        let files =
                            crate::tools::cursor::composer_data::files_edited_in_session(
                                sid,
                                &project_root,
                                0,
                            );
                        if !files.is_empty() {
                            let _ = state::snapshot_session_files(&project_root, sid, &files);
                        }
                    } else {
                        // Non-Cursor agents (Claude Code, Gemini, etc.): snapshot all
                        // dirty files in the worktree so the interceptor can compute
                        // precise AI vs human attribution at commit time.
                        let files = dirty_worktree_files(&project_root);
                        if !files.is_empty() {
                            let _ = state::snapshot_session_files(&project_root, sid, &files);
                        }
                    }
                }
            }
        }
        "subagent-stop" => {
            if let Some(sid) = session_id_field {
                state::touch_session(&project_root, sid, None)?;

                // subagentStop payload may contain modified_files directly.
                if !project_root.is_empty() {
                    if let Some(files_val) = event.extra.get("modified_files") {
                        if let Some(files_arr) = files_val.as_array() {
                            let files: Vec<String> = files_arr
                                .iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
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
    matches!(
        agent,
        "cursor" | "agent" | "composer" | "ask" | "edit" | "normal" | "chat"
    )
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
}
