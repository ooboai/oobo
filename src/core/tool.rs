use std::path::{Path, PathBuf};

use crate::analytics::NativeStats;
use crate::config::Config;
use crate::core::message::Message;
use crate::core::session::Session;

/// Every AI tool integration implements this trait.
///
/// Adding a new tool: create a file in `tools/`, implement `Tool`,
/// and add one line to the registry in `ToolRegistry::new()`.
pub trait Tool: Send + Sync {
    /// Identifier used in session.source (e.g. "cursor", "claude", "gemini").
    fn name(&self) -> &'static str;

    /// Human-readable display name (e.g. "Cursor", "Claude Code", "Gemini CLI").
    fn display_name(&self) -> &'static str;

    /// Config section key (e.g. "cursor", "claude"). Used for config field mapping.
    fn config_key(&self) -> &'static str;

    /// Tool category: "ide" for editor-embedded tools, "cli" for terminal agents.
    fn category(&self) -> &'static str;

    /// Whether this tool is enabled in the user's config.
    fn enabled(&self, cfg: &Config) -> bool;

    /// Discover sessions associated with a specific project.
    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String>;

    /// Discover all sessions across all projects.
    fn all_sessions(&self) -> Result<Vec<Session>, String>;

    /// Locate the transcript file for a session.
    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf>;

    /// Parse structured messages from a transcript file.
    fn parse_messages(&self, path: &Path) -> Vec<Message>;

    /// Parse messages by session ID directly (for tools that store messages in a DB).
    /// Override this for tools where `parse_messages(path)` doesn't work because
    /// the transcript path doesn't encode enough information.
    fn parse_messages_by_id(&self, project_path: &str, session_id: &str) -> Vec<Message> {
        if let Some(path) = self.find_transcript(project_path, session_id) {
            self.parse_messages(&path)
        } else {
            Vec::new()
        }
    }

    /// Extract native telemetry (tokens, model, cost) if the tool provides it.
    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        let _ = session;
        None
    }
}

/// Whether the given agent name refers to a Cursor session.
/// Cursor hooks may report the agent as any of these mode names.
pub fn is_cursor_agent(agent: &str) -> bool {
    matches!(
        agent,
        "cursor" | "agent" | "composer" | "ask" | "edit" | "normal" | "chat"
    )
}

/// Normalize an agent name to its canonical source identifier for DB storage.
/// Cursor's various mode names ("agent", "ask", "edit", etc.) all map to "composer".
pub fn normalize_source(agent: &str) -> &str {
    if is_cursor_agent(agent) {
        "composer"
    } else {
        agent
    }
}

/// Central registry of all supported AI tools.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self { tools }
    }

    /// Iterate over all registered tools.
    pub fn all(&self) -> impl Iterator<Item = &dyn Tool> {
        self.tools.iter().map(std::convert::AsRef::as_ref)
    }

    /// Iterate over tools that are enabled in config.
    pub fn enabled<'a>(&'a self, cfg: &'a Config) -> impl Iterator<Item = &'a dyn Tool> {
        self.tools
            .iter()
            .filter(move |t| t.enabled(cfg))
            .map(std::convert::AsRef::as_ref)
    }

    /// Look up a tool by its source name or config key.
    /// This handles the Cursor case where name() is "composer" but config_key() is "cursor".
    pub fn by_name(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name || t.config_key() == name)
            .map(std::convert::AsRef::as_ref)
    }

    /// All (name, display_name) pairs  --  single source of truth for tool lists.
    #[cfg(test)]
    pub fn tool_names(&self) -> Vec<(&'static str, &'static str)> {
        self.tools
            .iter()
            .map(|t| (t.name(), t.display_name()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cursor_agent() {
        assert!(is_cursor_agent("cursor"));
        assert!(is_cursor_agent("agent"));
        assert!(is_cursor_agent("composer"));
        assert!(is_cursor_agent("ask"));
        assert!(is_cursor_agent("edit"));
        assert!(is_cursor_agent("normal"));
        assert!(is_cursor_agent("chat"));
        assert!(!is_cursor_agent("claude"));
        assert!(!is_cursor_agent("gemini"));
        assert!(!is_cursor_agent("codex"));
        assert!(!is_cursor_agent(""));
    }
}
