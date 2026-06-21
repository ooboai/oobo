use std::fs;
use std::path::{Path, PathBuf};

use super::cursor_support_dir;

/// Find all workspace directories that match the given project root.
pub fn find_workspace_dirs_for_project(
    project_root: &str,
) -> Result<Vec<(PathBuf, String)>, String> {
    let ws_storage = workspace_storage_dir().ok_or("Cursor workspace storage not found")?;
    if !ws_storage.exists() {
        return Ok(Vec::new());
    }

    let norm_root = normalize_path(project_root);
    let mut matches = Vec::new();

    let entries = fs::read_dir(&ws_storage)
        .map_err(|e| format!("cannot read {}: {e}", ws_storage.display()))?;

    for entry in entries.flatten() {
        let ws_dir = entry.path();
        if let Some(folder_path) = read_workspace_folder(&ws_dir) {
            if normalize_path(&folder_path) == norm_root {
                matches.push((ws_dir, folder_path));
            }
        }
    }

    Ok(matches)
}

/// Find all workspace directories with their project paths.
pub fn find_all_workspace_dirs() -> Result<Vec<(PathBuf, String)>, String> {
    let ws_storage = workspace_storage_dir().ok_or("Cursor workspace storage not found")?;
    if !ws_storage.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();

    let entries = fs::read_dir(&ws_storage)
        .map_err(|e| format!("cannot read {}: {e}", ws_storage.display()))?;

    for entry in entries.flatten() {
        let ws_dir = entry.path();
        if let Some(folder_path) = read_workspace_folder(&ws_dir) {
            results.push((ws_dir, folder_path));
        }
    }

    Ok(results)
}

fn workspace_storage_dir() -> Option<PathBuf> {
    cursor_support_dir().map(|d| d.join("User/workspaceStorage"))
}

/// Read workspace.json to extract the folder path.
fn read_workspace_folder(ws_dir: &Path) -> Option<String> {
    let ws_json = ws_dir.join("workspace.json");
    let content = fs::read_to_string(ws_json).ok()?;
    let data: serde_json::Value = serde_json::from_str(&content).ok()?;
    let folder_uri = data.get("folder")?.as_str()?;
    Some(uri_to_path(folder_uri))
}

/// Convert a file:// URI to a filesystem path.
fn uri_to_path(uri: &str) -> String {
    if let Ok(url) = url::Url::parse(uri) {
        url.to_file_path()
            .map_or_else(|()| uri.to_string(), |p| p.to_string_lossy().to_string())
    } else {
        uri.to_string()
    }
}

fn normalize_path(p: &str) -> String {
    let path = Path::new(p);
    match fs::canonicalize(path) {
        Ok(canonical) => crate::utils::normalize_win_path(&canonical.to_string_lossy()).to_string(),
        Err(_) => p.trim_end_matches('/').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_to_path() {
        #[cfg(unix)]
        assert_eq!(
            uri_to_path("file:///home/user/projects/my-app"),
            "/home/user/projects/my-app"
        );
        #[cfg(windows)]
        assert_eq!(
            uri_to_path("file:///C:/Users/user/projects/my-app"),
            "C:\\Users\\user\\projects\\my-app"
        );
    }

    #[test]
    fn test_uri_to_path_encoded() {
        #[cfg(unix)]
        assert_eq!(
            uri_to_path("file:///home/user/My%20Projects/app"),
            "/home/user/My Projects/app"
        );
        #[cfg(windows)]
        assert_eq!(
            uri_to_path("file:///C:/Users/user/My%20Projects/app"),
            "C:\\Users\\user\\My Projects\\app"
        );
    }
}
