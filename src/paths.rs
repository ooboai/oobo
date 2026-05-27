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

/// The root oobo configuration directory.
///
/// Resolution order:
/// 1. `$OOBO_HOME` (explicit override)
/// 2. `~/.oobo/` (legacy  --  used if it already exists)
/// 3. `~/.config/oobo/` (XDG default)
pub fn oobo_home() -> PathBuf {
    if let Ok(v) = std::env::var("OOBO_HOME") {
        return PathBuf::from(v);
    }
    let home = dirs::home_dir()
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    let legacy = home.join(".oobo");
    if legacy.exists() {
        return legacy;
    }

    home.join(".config").join("oobo")
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
        // Other tests (e.g. hooks::state) may transiently override
        // OOBO_HOME to a tempdir for isolation. Only assert the default
        // shape when OOBO_HOME is unset.
        if std::env::var_os("OOBO_HOME").is_some() {
            return;
        }
        let home = oobo_home();
        let s = home.to_string_lossy();
        assert!(s.contains("oobo"), "expected 'oobo' in path: {s}");
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
