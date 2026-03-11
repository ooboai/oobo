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
/// Called by `oobo hooks agent <event>` which receives JSON on stdin.
/// This is internal plumbing — never typed by the user.
pub fn handle_event(event_name: &str, payload: &str) -> crate::error::Result<()> {
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

    let raw_agent = event
        .agent
        .as_deref()
        .or(event.extra.get("composer_mode").and_then(|v| v.as_str()))
        .unwrap_or("cursor");

    let agent = normalize_agent_name(raw_agent);

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
        }
        "session-end" => {
            if let Some(sid) = session_id_field {
                state::remove_session(&project_root, sid);
            }
        }
        "stop" => {
            if let Some(sid) = session_id_field {
                let transcript_path = event
                    .extra
                    .get("transcript_path")
                    .and_then(|v| v.as_str());
                state::touch_session(&project_root, sid, transcript_path)?;
            }
        }
        _ => {}
    }

    Ok(())
}

/// Normalize agent names from hook payloads to canonical tool names.
/// Cursor sends "agent" or "composer" as the mode name — map to "cursor".
fn normalize_agent_name(raw: &str) -> &str {
    match raw {
        "agent" | "composer" | "ask" | "edit" | "normal" | "chat" => "cursor",
        other => other,
    }
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
    fn test_normalize_agent_name() {
        assert_eq!(normalize_agent_name("agent"), "cursor");
        assert_eq!(normalize_agent_name("composer"), "cursor");
        assert_eq!(normalize_agent_name("ask"), "cursor");
        assert_eq!(normalize_agent_name("edit"), "cursor");
        assert_eq!(normalize_agent_name("cursor"), "cursor");
        assert_eq!(normalize_agent_name("claude"), "claude");
        assert_eq!(normalize_agent_name("gemini"), "gemini");
        assert_eq!(normalize_agent_name("aider"), "aider");
    }

    #[test]
    fn test_handle_event_session_start() {
        let result = handle_event("session-start", "{}");
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_event_invalid_json() {
        let result = handle_event("test", "not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_event_session_end_no_id() {
        let result = handle_event("session-end", "{}");
        assert!(result.is_ok());
    }
}
