use std::fs;
use std::path::{Path, PathBuf};

/// Convert a filesystem path to a slug suitable for directory names.
///
/// Handles both Unix `/` and Windows `\` separators, producing a
/// consistent slug across platforms.
///
/// `/Users/alice/dev/project` → `Users-alice-dev-project`
/// `C:\Users\alice\project`  → `C-Users-alice-project`
pub fn slug_from_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .replace('/', "-")
        .replace(':', "")
}

/// Attempt to canonicalize a path, falling back to a cleaned version.
pub fn normalize_path(p: &str) -> String {
    match fs::canonicalize(p) {
        Ok(canonical) => canonical.to_string_lossy().to_string(),
        Err(_) => p.replace('\\', "/").trim_end_matches('/').to_string(),
    }
}

/// Get the git project root for the current working directory.
pub fn git_project_root() -> String {
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
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        })
}

/// The root oobo configuration directory (`~/.oobo`).
pub fn oobo_home() -> PathBuf {
    if let Ok(v) = std::env::var("OOBO_HOME") {
        return PathBuf::from(v);
    }
    dirs::home_dir()
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".oobo")
}

/// Directory for the oobo SQLite database (`~/.oobo/db/`).
pub fn oobo_db_dir() -> PathBuf {
    oobo_home().join("db")
}

/// Path to the oobo SQLite database file (`~/.oobo/db/oobo.db`).
pub fn oobo_db_path() -> PathBuf {
    oobo_db_dir().join("oobo.db")
}

/// Directory for per-project data (`~/.oobo/projects/`).
pub fn oobo_projects_dir() -> PathBuf {
    oobo_home().join("projects")
}

/// Directory for a specific project (`~/.oobo/projects/{slug}/`).
pub fn oobo_project_dir(project_path: &str) -> PathBuf {
    oobo_projects_dir().join(slug_from_path(project_path))
}

/// Ensure a directory exists, creating it and all parents if needed.
pub fn ensure_dir(path: &Path) -> Result<(), String> {
    if !path.exists() {
        fs::create_dir_all(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slug_unix_absolute() {
        assert_eq!(
            slug_from_path("/Users/alice/dev/projects/oobo/oobo-cli"),
            "Users-alice-dev-projects-oobo-oobo-cli"
        );
    }

    #[test]
    fn test_slug_unix_root() {
        assert_eq!(slug_from_path("/tmp"), "tmp");
    }

    #[test]
    fn test_slug_windows_absolute() {
        assert_eq!(
            slug_from_path("C:\\Users\\alice\\project"),
            "C-Users-alice-project"
        );
    }

    #[test]
    fn test_slug_trailing_slash() {
        assert_eq!(slug_from_path("/home/user/project/"), "home-user-project-");
    }

    #[test]
    fn test_slug_relative() {
        assert_eq!(slug_from_path("src/main.rs"), "src-main.rs");
    }

    #[test]
    fn test_slug_empty() {
        assert_eq!(slug_from_path(""), "");
    }

    #[test]
    fn test_normalize_nonexistent() {
        let result = normalize_path("/nonexistent/path/abc123");
        assert_eq!(result, "/nonexistent/path/abc123");
    }

    #[test]
    fn test_normalize_strips_trailing() {
        let result = normalize_path("/nonexistent/path/");
        assert_eq!(result, "/nonexistent/path");
    }

    #[test]
    fn test_oobo_home_is_under_home() {
        let home = oobo_home();
        assert!(home.to_string_lossy().contains(".oobo"));
    }

    #[test]
    fn test_oobo_db_path_structure() {
        let db = oobo_db_path();
        assert!(db.to_string_lossy().contains("db"));
        assert!(db.to_string_lossy().ends_with("oobo.db"));
    }

    #[test]
    fn test_oobo_project_dir() {
        let dir = oobo_project_dir("/Users/alice/dev/project");
        assert!(dir.to_string_lossy().ends_with("Users-alice-dev-project"));
    }

    #[test]
    fn test_ensure_dir_creates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        assert!(!nested.exists());
        ensure_dir(&nested).unwrap();
        assert!(nested.exists());
    }
}
