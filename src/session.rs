use crate::core::message::Message;
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
#[cfg(test)]
pub fn find_transcript_path(session: &crate::core::session::Session) -> Option<std::path::PathBuf> {
    let r = reg();
    let tool = tool_or_default(&r, &session.source)?;
    tool.find_transcript(&session.project_path, &session.session_id)
}

/// Parse messages from a transcript file.
#[cfg(test)]
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

/// Get all sessions across all sources for the current project.
#[cfg(test)]
pub fn all_for_project(project_root: &str, cfg: &crate::config::Config) -> Vec<crate::core::session::Session> {
    let r = reg();
    let mut sessions: Vec<crate::core::session::Session> = r
        .enabled(cfg)
        .flat_map(|tool| tool.sessions_for_project(project_root).unwrap_or_default())
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    sessions
}

/// Get all sessions across all sources and all projects.
#[cfg(test)]
pub fn all_sessions(cfg: &crate::config::Config) -> Vec<crate::core::session::Session> {
    let r = reg();
    let mut sessions: Vec<crate::core::session::Session> = r
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
}
