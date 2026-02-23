pub mod composer;
pub mod transcript;
pub mod workspace;

use std::path::PathBuf;

/// A Cursor chat session with metadata.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub name: String,
    pub mode: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub project_path: String,
    #[allow(dead_code)]
    pub workspace_dir: String,
    pub source: String,
}

impl Session {
    pub fn sort_key(&self) -> i64 {
        self.updated_at.or(self.created_at).unwrap_or(0)
    }

    pub fn short_id(&self) -> &str {
        if self.session_id.len() >= 8 {
            &self.session_id[..8]
        } else {
            &self.session_id
        }
    }

    pub fn updated_at_iso(&self) -> String {
        epoch_ms_to_iso(self.updated_at)
    }

    pub fn created_at_iso(&self) -> String {
        epoch_ms_to_iso(self.created_at)
    }
}

fn epoch_ms_to_iso(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => {
            let secs = ms / 1000;
            match chrono::DateTime::from_timestamp(secs, 0) {
                Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                None => String::new(),
            }
        }
        None => String::new(),
    }
}

/// Platform-specific Cursor application support directory.
pub fn cursor_support_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/Cursor"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::config_dir().map(|c| c.join("Cursor"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|d| d.join("Cursor"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Directory where Cursor stores per-project data (agent transcripts, etc.)
pub fn cursor_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".cursor/projects"))
}

/// Convert a filesystem path to the Cursor project slug format.
/// `/home/user/projects/my-app` → `home-user-projects-my-app`
pub fn path_to_slug(path: &str) -> String {
    path.trim_start_matches('/').replace('/', "-")
}

/// Get the project root for the current directory.
pub fn get_project_root() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        })
}

/// Get all sessions for a given project root, sorted by most recent first.
pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let ws_dirs = workspace::find_workspace_dirs_for_project(project_root)?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        sessions.extend(composer::extract_sessions(ws_dir, folder_path));
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

/// Get all sessions across all projects.
pub fn all_sessions() -> Result<Vec<Session>, String> {
    let ws_dirs = workspace::find_all_workspace_dirs()?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        sessions.extend(composer::extract_sessions(ws_dir, folder_path));
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

/// Find a session by ID prefix, searching current project first, then all.
#[allow(dead_code)]
pub fn find_session(id_prefix: &str) -> Result<Session, String> {
    let project_root = get_project_root();

    if let Ok(sessions) = sessions_for_project(&project_root) {
        if let Some(s) = sessions
            .iter()
            .find(|s| s.session_id.starts_with(id_prefix))
        {
            return Ok(s.clone());
        }
    }

    let all = all_sessions()?;
    all.into_iter()
        .find(|s| s.session_id.starts_with(id_prefix))
        .ok_or_else(|| format!("session not found: {id_prefix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_slug() {
        assert_eq!(
            path_to_slug("/home/user/projects/my-app"),
            "home-user-projects-my-app"
        );
        assert_eq!(path_to_slug("/tmp"), "tmp");
    }

    #[test]
    fn test_epoch_ms_to_iso() {
        assert_eq!(epoch_ms_to_iso(None), "");
        assert_eq!(epoch_ms_to_iso(Some(1708617600000)), "2024-02-22T16:00:00");
    }

    #[test]
    fn test_session_short_id() {
        let s = Session {
            session_id: "2c97dced-3950-482e-b101-9eb7d1b18cf5".into(),
            name: "test".into(),
            mode: "agent".into(),
            created_at: Some(1000),
            updated_at: Some(2000),
            project_path: "/tmp".into(),
            workspace_dir: "/tmp".into(),
            source: "composer".into(),
        };
        assert_eq!(s.short_id(), "2c97dced");
    }
}
