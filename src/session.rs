use std::path::PathBuf;

use crate::config::Config;
use crate::core::message::Message;
use crate::core::session::Session;
use crate::core::tool::ToolRegistry;
use crate::tools;

fn reg() -> ToolRegistry {
    tools::registry()
}

fn tool_or_default<'a>(
    registry: &'a ToolRegistry,
    source: &str,
) -> Option<&'a dyn crate::core::tool::Tool> {
    registry
        .by_name(source)
        .or_else(|| registry.by_name("composer"))
}

/// Find the transcript path for a session.
pub fn find_transcript_path(session: &Session) -> Option<PathBuf> {
    let r = reg();
    let tool = tool_or_default(&r, &session.source)?;
    tool.find_transcript(&session.project_path, &session.session_id)
}

/// Count messages in a session's transcript.
pub fn count_messages(session: &Session) -> u32 {
    let r = reg();
    match tool_or_default(&r, &session.source) {
        Some(tool) => tool.count_messages(&session.project_path, &session.session_id),
        None => 0,
    }
}

/// Parse messages from a transcript file.
pub fn parse_messages(path: &std::path::Path, source: &str) -> Vec<Message> {
    let r = reg();
    match tool_or_default(&r, source) {
        Some(tool) => tool.parse_messages(path),
        None => Vec::new(),
    }
}

/// Parse messages by session ID (uses `parse_messages_by_id` for tools that override it).
pub fn parse_messages_for_session(
    project_path: &str,
    session_id: &str,
    source: &str,
) -> Vec<Message> {
    let r = reg();
    match tool_or_default(&r, source) {
        Some(tool) => tool.parse_messages_by_id(project_path, session_id),
        None => Vec::new(),
    }
}

/// Read transcript as formatted text.
pub fn read_transcript(path: &std::path::Path, max_messages: u32, source: &str) -> String {
    let r = reg();
    match tool_or_default(&r, source) {
        Some(tool) => tool.read_transcript(path, max_messages),
        None => String::new(),
    }
}

/// Read transcript by session ID (uses `read_transcript_by_id` for tools that override it).
pub fn read_transcript_for_session(
    project_path: &str,
    session_id: &str,
    max_messages: u32,
    source: &str,
) -> String {
    let r = reg();
    match tool_or_default(&r, source) {
        Some(tool) => tool.read_transcript_by_id(project_path, session_id, max_messages),
        None => String::new(),
    }
}

/// Find a session by ID prefix across all sources.
/// Searches project-scoped sessions first, then falls back to all sessions.
pub fn find_session_any(id_prefix: &str) -> Result<Session, String> {
    let r = reg();
    let project_root = crate::paths::git_project_root();

    for tool in r.all() {
        if let Ok(sessions) = tool.sessions_for_project(&project_root) {
            if let Some(s) = sessions
                .iter()
                .find(|s| s.session_id.starts_with(id_prefix))
            {
                return Ok(s.clone());
            }
        }
    }

    for tool in r.all() {
        if let Ok(sessions) = tool.all_sessions() {
            if let Some(s) = sessions
                .iter()
                .find(|s| s.session_id.starts_with(id_prefix))
            {
                return Ok(s.clone());
            }
        }
    }

    Err(format!("session not found: {id_prefix}"))
}

/// Get all sessions across all sources for the current project.
pub fn all_for_project(project_root: &str, cfg: &Config) -> Vec<Session> {
    let r = reg();
    let mut sessions: Vec<Session> = r
        .enabled(cfg)
        .flat_map(|tool| tool.sessions_for_project(project_root).unwrap_or_default())
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    sessions
}

/// Get all sessions across all sources and all projects.
pub fn all_sessions(cfg: &Config) -> Vec<Session> {
    let r = reg();
    let mut sessions: Vec<Session> = r
        .enabled(cfg)
        .flat_map(|tool| tool.all_sessions().unwrap_or_default())
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_session_any_nonexistent() {
        let result = find_session_any("zzz-does-not-exist-99999");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("session not found"),
            "expected 'session not found', got: {err}"
        );
    }

    #[test]
    fn test_tool_or_default_known_source() {
        let r = reg();
        let tool = tool_or_default(&r, "composer");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "composer");
    }

    #[test]
    fn test_tool_or_default_claude_source() {
        let r = reg();
        let tool = tool_or_default(&r, "claude");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "claude");
    }

    #[test]
    fn test_tool_or_default_unknown_falls_back_to_composer() {
        let r = reg();
        let tool = tool_or_default(&r, "totally-unknown-tool");
        assert!(
            tool.is_some(),
            "unknown source should fall back to composer"
        );
        assert_eq!(tool.unwrap().name(), "composer");
    }

    #[test]
    fn test_parse_messages_nonexistent_path() {
        let msgs = parse_messages(
            std::path::Path::new("/tmp/nonexistent-oobo-transcript.jsonl"),
            "claude",
        );
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_count_messages_nonexistent() {
        let session = Session {
            session_id: "fake-session-xyz".into(),
            source: "claude".into(),
            project_path: "/tmp/nonexistent-project".into(),
            name: String::new(),
            mode: String::new(),
            workspace_dir: String::new(),
            created_at: None,
            updated_at: None,
        };
        assert_eq!(count_messages(&session), 0);
    }

    #[test]
    fn test_read_transcript_nonexistent() {
        let text = read_transcript(
            std::path::Path::new("/tmp/no-such-transcript.jsonl"),
            10,
            "composer",
        );
        assert!(text.is_empty());
    }
}
