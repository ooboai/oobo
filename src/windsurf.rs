use crate::cursor::Session;
use crate::vscode_fork::{self, ForkConfig};

const CONFIG: ForkConfig = ForkConfig {
    app_name: "Windsurf",
    dot_dir: "windsurf",
    composer_keys: &["composer.composerData", "cascade.composerData"],
    source: "windsurf",
};

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    vscode_fork::sessions_for_project(&CONFIG, project_root)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    vscode_fork::all_sessions(&CONFIG)
}

pub mod transcript {
    use std::path::PathBuf;

    use crate::cursor::transcript::Message;

    pub fn find_transcript_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
        super::vscode_fork::find_transcript_path("windsurf", project_path, session_id)
    }

    pub fn count_messages(project_path: &str, session_id: &str) -> u32 {
        super::vscode_fork::count_messages("windsurf", project_path, session_id)
    }

    pub fn parse_messages(path: &std::path::Path) -> Vec<Message> {
        crate::cursor::transcript::parse_messages(path)
    }

    pub fn read_transcript(path: &std::path::Path, max_messages: u32) -> String {
        crate::cursor::transcript::read_transcript(path, max_messages)
    }
}
