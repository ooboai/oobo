pub mod ai_tracking;
pub mod composer;
pub mod composer_data;
pub mod transcript;
pub mod workspace;

use std::path::PathBuf;

pub use crate::core::session::Session;

/// Path to Cursor's global `state.vscdb` SQLite database.
pub fn state_vscdb_path() -> Option<PathBuf> {
    cursor_support_dir().map(|d| d.join("User/globalStorage/state.vscdb"))
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

#[allow(dead_code)]
pub fn get_project_root() -> String {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    std::process::Command::new(git)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .replace('\r', "")
                .trim()
                .to_string()
        })
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        })
}

/// Get all sessions for a given project root, sorted by most recent first.
///
/// Checks per-workspace legacy storage first, then falls back to the global
/// `state.vscdb` where Cursor now stores individual composer records.
pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let ws_dirs = workspace::find_workspace_dirs_for_project(project_root)?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        sessions.extend(composer::extract_sessions(ws_dir, folder_path));
    }
    dedup_sessions(&mut sessions);
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

/// Get all sessions across all projects.
///
/// Merges legacy per-workspace data with the global DB to cover both old
/// and new Cursor storage formats.
pub fn all_sessions() -> Result<Vec<Session>, String> {
    let ws_dirs = workspace::find_all_workspace_dirs()?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        sessions.extend(composer::extract_sessions(ws_dir, folder_path));
    }
    let global = composer::extract_all_sessions_global();
    sessions.extend(global);
    dedup_sessions(&mut sessions);
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

fn dedup_sessions(sessions: &mut Vec<Session>) {
    let mut seen = std::collections::HashSet::new();
    sessions.retain(|s| seen.insert(s.session_id.clone()));
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
}
