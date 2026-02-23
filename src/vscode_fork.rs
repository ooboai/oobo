use std::fs;
use std::path::{Path, PathBuf};

use crate::cursor::Session;

/// Configuration for a VS Code fork (Cursor, Windsurf, Trae, etc.).
pub struct ForkConfig {
    pub app_name: &'static str,
    #[allow(dead_code)]
    pub dot_dir: &'static str,
    pub composer_keys: &'static [&'static str],
    pub source: &'static str,
}

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

fn projects_dir(dot_dir: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(format!(".{dot_dir}/projects")))
}

fn path_to_slug(path: &str) -> String {
    path.trim_start_matches('/').replace('/', "-")
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

    let conn = match rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
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

// ── Transcript helpers ──────────────────────────────────────────────────────

/// Find a transcript file in the fork's projects directory.
pub fn find_transcript_path(
    dot_dir: &str,
    project_path: &str,
    session_id: &str,
) -> Option<PathBuf> {
    let projects = projects_dir(dot_dir)?;
    let slug = path_to_slug(project_path);
    let transcripts_dir = projects.join(slug).join("agent-transcripts");

    let subdir = transcripts_dir.join(session_id);
    if subdir.is_dir() {
        let jsonl = subdir.join(format!("{session_id}.jsonl"));
        if jsonl.exists() {
            return Some(jsonl);
        }
    }

    let txt = transcripts_dir.join(format!("{session_id}.txt"));
    if txt.exists() {
        return Some(txt);
    }

    if transcripts_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&transcripts_dir) {
            let prefix = &session_id[..session_id.len().min(8)];
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if entry.path().is_dir() && name_str.starts_with(prefix) {
                    let jsonl = entry.path().join(format!("{name_str}.jsonl"));
                    if jsonl.exists() {
                        return Some(jsonl);
                    }
                } else if entry.path().is_file() {
                    let stem = Path::new(&*name_str)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if stem.starts_with(prefix) {
                        return Some(entry.path());
                    }
                }
            }
        }
    }

    None
}

pub fn count_messages(dot_dir: &str, project_path: &str, session_id: &str) -> u32 {
    match find_transcript_path(dot_dir, project_path, session_id) {
        Some(p) => crate::cursor::transcript::count_messages_in_file(&p),
        None => 0,
    }
}

// ── High-level session functions ────────────────────────────────────────────

pub fn sessions_for_project(
    config: &ForkConfig,
    project_root: &str,
) -> Result<Vec<Session>, String> {
    let ws_dirs = find_workspace_dirs_for_project(config.app_name, project_root)?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        sessions.extend(extract_sessions(
            ws_dir,
            folder_path,
            config.composer_keys,
            config.source,
        ));
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions(config: &ForkConfig) -> Result<Vec<Session>, String> {
    let ws_dirs = find_all_workspace_dirs(config.app_name)?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        sessions.extend(extract_sessions(
            ws_dir,
            folder_path,
            config.composer_keys,
            config.source,
        ));
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_slug() {
        assert_eq!(path_to_slug("/Users/dev/project"), "Users-dev-project");
    }

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
