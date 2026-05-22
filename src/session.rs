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
}
