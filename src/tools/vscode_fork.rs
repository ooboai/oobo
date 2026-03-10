use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::cursor::Session;

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
        url.to_file_path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| uri.to_string())
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

/// Extract sessions from state.vscdb, trying each composer key in order.
#[allow(dead_code)]
pub fn extract_sessions(
    ws_dir: &Path,
    project_path: &str,
    composer_keys: &[&str],
    source: &str,
) -> Vec<Session> {
    let db_path = ws_dir.join("state.vscdb");
    if !db_path.exists() {
        return Vec::new();
    }

    let conn = match crate::utils::open_db_readonly(&db_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    for key in composer_keys {
        let sessions = try_extract_with_key(&conn, key, ws_dir, project_path, source);
        if !sessions.is_empty() {
            return sessions;
        }
    }

    Vec::new()
}

fn try_extract_with_key(
    conn: &rusqlite::Connection,
    key: &str,
    ws_dir: &Path,
    project_path: &str,
    source: &str,
) -> Vec<Session> {
    let raw: String =
        match conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| {
            row.get(0)
        }) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

    let data: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let composers = match data.get("allComposers").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for c in composers {
        let cid = match c.get("composerId").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };

        sessions.push(Session {
            session_id: cid,
            name: c
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            mode: c
                .get("unifiedMode")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            created_at: c.get("createdAt").and_then(|v| v.as_i64()),
            updated_at: c.get("lastUpdatedAt").and_then(|v| v.as_i64()),
            project_path: project_path.to_string(),
            workspace_dir: ws_dir.to_string_lossy().to_string(),
            source: source.to_string(),
        });
    }

    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sessions_missing_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sessions = extract_sessions(tmp.path(), "/tmp", &["composer.composerData"], "test");
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_extract_sessions_multiple_keys() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("state.vscdb");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            [
                "cascade.composerData",
                r#"{"allComposers":[{"composerId":"ws-1","name":"Cascade Chat","unifiedMode":"chat"}]}"#,
            ],
        )
        .unwrap();

        let sessions = extract_sessions(
            tmp.path(),
            "/tmp",
            &["composer.composerData", "cascade.composerData"],
            "windsurf",
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].source, "windsurf");
        assert_eq!(sessions[0].name, "Cascade Chat");
    }
}
