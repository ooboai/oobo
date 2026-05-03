use std::fs;
use std::path::{Path, PathBuf};

/// Platform-specific application support directory.
pub fn support_dir(app_name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join(format!("Library/Application Support/{app_name}")))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::config_dir().map(|c| c.join(app_name))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir().map(|d| d.join(app_name))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

fn workspace_storage_dir(app_name: &str) -> Option<PathBuf> {
    support_dir(app_name).map(|d| d.join("User/workspaceStorage"))
}

// ── Workspace scanning ──────────────────────────────────────────────────────

pub fn find_workspace_dirs_for_project(
    app_name: &str,
    project_root: &str,
) -> Result<Vec<(PathBuf, String)>, String> {
    let ws_storage =
        workspace_storage_dir(app_name).ok_or_else(|| format!("{app_name} not found"))?;
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

pub fn find_all_workspace_dirs(app_name: &str) -> Result<Vec<(PathBuf, String)>, String> {
    let ws_storage =
        workspace_storage_dir(app_name).ok_or_else(|| format!("{app_name} not found"))?;
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

fn read_workspace_folder(ws_dir: &Path) -> Option<String> {
    let ws_json = ws_dir.join("workspace.json");
    let content = fs::read_to_string(ws_json).ok()?;
    let data: serde_json::Value = serde_json::from_str(&content).ok()?;
    let folder_uri = data.get("folder")?.as_str()?;
    Some(uri_to_path(folder_uri))
}

fn uri_to_path(uri: &str) -> String {
    if let Ok(url) = url::Url::parse(uri) {
        url.to_file_path().map_or_else(|()| uri.to_string(), |p| p.to_string_lossy().to_string())
    } else {
        uri.to_string()
    }
}

fn normalize_path(p: &str) -> String {
    match fs::canonicalize(p) {
        Ok(canonical) => canonical.to_string_lossy().to_string(),
        Err(_) => p.trim_end_matches('/').to_string(),
    }
}

// ── Composer extraction ─────────────────────────────────────────────────────


#[cfg(test)]
mod tests {
}
