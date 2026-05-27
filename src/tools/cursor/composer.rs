use std::path::Path;

use super::Session;

const COMPOSER_KEY: &str = "composer.composerData";

/// Map Cursor's numeric subagent type IDs to human-readable names.
/// These values come from Cursor's internal `SubagentType` enum in
/// composer data (observed via state.vscdb inspection). IDs 2 and 3
/// are both explore variants (quick/thorough)  --  collapsed to "explore".
fn map_subagent_type(type_id: u64) -> String {
    match type_id {
        0 => "generalPurpose".to_string(),
        1 => "shell".to_string(),
        2 | 3 => "explore".to_string(),
        4 => "browser-use".to_string(),
        5 => "best-of-n-runner".to_string(),
        _ => format!("unknown-{type_id}"),
    }
}

/// Extract sessions from a workspace's state.vscdb.
///
/// Tries two storage formats in order:
/// 1. **Legacy** (pre-2026): `composer.composerData` → `allComposers` array in the
///    per-workspace `state.vscdb` (ItemTable).
/// 2. **Current**: Individual `composerData:<id>` rows in the **global** `state.vscdb`
///    (`cursorDiskKV` table), filtered by `workspaceIdentifier.uri.fsPath`.
pub fn extract_sessions(ws_dir: &Path, project_path: &str) -> Vec<Session> {
    let legacy = extract_sessions_legacy(ws_dir, project_path);
    if !legacy.is_empty() {
        return legacy;
    }

    extract_sessions_global(project_path, ws_dir)
}

/// Legacy format: all composers in a single JSON blob per workspace.
fn extract_sessions_legacy(ws_dir: &Path, project_path: &str) -> Vec<Session> {
    let db_path = ws_dir.join("state.vscdb");
    if !db_path.exists() {
        return Vec::new();
    }

    let conn = match crate::utils::open_db_readonly(&db_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let raw: String = match conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ?1",
        [COMPOSER_KEY],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let data: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let composers = match data.get("allComposers").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Vec::new(),
    };

    composers
        .iter()
        .filter_map(|c| parse_composer_value(c, project_path, &ws_dir.to_string_lossy()))
        .collect()
}

/// Current format: individual composerData:<id> rows in global state.vscdb.
///
/// Uses targeted key lookups instead of table scans. First reads known
/// composer IDs from the workspace's `composer.composerData` pointer,
/// then fetches each by exact key from the global DB. Also resolves
/// subagent IDs recursively.
fn extract_sessions_global(project_path: &str, ws_dir: &Path) -> Vec<Session> {
    let global_db = match super::state_vscdb_path() {
        Some(p) if p.exists() => p,
        _ => return Vec::new(),
    };

    let seed_ids = read_workspace_composer_ids(ws_dir);
    if seed_ids.is_empty() {
        return Vec::new();
    }

    let conn = match crate::utils::open_db_readonly(&global_db) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let ws_dir_str = ws_dir.to_string_lossy().to_string();
    let mut sessions = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<String> = seed_ids.into_iter().collect();

    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }

        let key = format!("composerData:{id}");
        let raw: String = match conn.query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?1",
            [&key],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let val: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(subs) = val.get("subagentComposerIds").and_then(|v| v.as_array()) {
            for sub in subs {
                if let Some(sub_id) = sub.as_str() {
                    queue.push_back(sub_id.to_string());
                }
            }
        }
        if let Some(subs) = val.get("subComposerIds").and_then(|v| v.as_array()) {
            for sub in subs {
                if let Some(sub_id) = sub.as_str() {
                    queue.push_back(sub_id.to_string());
                }
            }
        }

        if let Some(session) = parse_composer_value(&val, project_path, &ws_dir_str) {
            sessions.push(session);
        }
    }

    sessions
}

/// Read composer IDs from the workspace's legacy pointer.
/// Returns `selectedComposerIds` + `lastFocusedComposerIds` merged.
fn read_workspace_composer_ids(ws_dir: &Path) -> Vec<String> {
    let db_path = ws_dir.join("state.vscdb");
    if !db_path.exists() {
        return Vec::new();
    }

    let conn = match crate::utils::open_db_readonly(&db_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let raw: String = match conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ?1",
        [COMPOSER_KEY],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let data: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut ids = std::collections::HashSet::new();
    for field in ["selectedComposerIds", "lastFocusedComposerIds"] {
        if let Some(arr) = data.get(field).and_then(|v| v.as_array()) {
            for id in arr {
                if let Some(s) = id.as_str() {
                    if !s.is_empty() {
                        ids.insert(s.to_string());
                    }
                }
            }
        }
    }

    ids.into_iter().collect()
}

/// Bulk-read all sessions across all projects from the global DB.
///
/// Iterates all workspace directories, collects their composer ID pointers,
/// then fetches each by exact key from the global DB.
pub fn extract_all_sessions_global() -> Vec<Session> {
    let global_db = match super::state_vscdb_path() {
        Some(p) if p.exists() => p,
        _ => return Vec::new(),
    };

    let ws_dirs = match super::workspace::find_all_workspace_dirs() {
        Ok(dirs) => dirs,
        Err(_) => return Vec::new(),
    };

    let conn = match crate::utils::open_db_readonly(&global_db) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (ws_dir, project_path) in &ws_dirs {
        let seed_ids = read_workspace_composer_ids(ws_dir);
        let ws_dir_str = ws_dir.to_string_lossy().to_string();

        let mut queue: std::collections::VecDeque<String> = seed_ids.into_iter().collect();
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }

            let key = format!("composerData:{id}");
            let raw: String = match conn.query_row(
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                [&key],
                |row| row.get(0),
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let val: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(subs) = val.get("subagentComposerIds").and_then(|v| v.as_array()) {
                for sub in subs {
                    if let Some(sub_id) = sub.as_str() {
                        queue.push_back(sub_id.to_string());
                    }
                }
            }

            if let Some(session) = parse_composer_value(&val, project_path, &ws_dir_str) {
                sessions.push(session);
            }
        }
    }

    sessions
}

// ── Shared helpers ───────────────────────────────────────────────────

fn parse_composer_value(
    c: &serde_json::Value,
    project_path: &str,
    ws_dir: &str,
) -> Option<Session> {
    let cid = c.get("composerId").and_then(|v| v.as_str())?;
    if cid.is_empty() {
        return None;
    }

    let (parent_id, subagent_tp) = c
        .get("subagentInfo")
        .and_then(|info| {
            if info.is_null() {
                return None;
            }
            let parent = info
                .get("parentComposerId")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);
            let stype = info
                .get("subagentType")
                .and_then(serde_json::Value::as_u64)
                .map(map_subagent_type);
            Some((parent, stype))
        })
        .unwrap_or((None, None));

    Some(Session {
        session_id: cid.to_string(),
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
        created_at: c.get("createdAt").and_then(serde_json::Value::as_i64),
        updated_at: c.get("lastUpdatedAt").and_then(serde_json::Value::as_i64),
        project_path: project_path.to_string(),
        workspace_dir: ws_dir.to_string(),
        source: "composer".to_string(),
        parent_session_id: parent_id,
        subagent_type: subagent_tp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn create_test_db(dir: &std::path::Path, composers_json: &str) {
        let db_path = dir.join("state.vscdb");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            [COMPOSER_KEY, composers_json],
        )
        .unwrap();
    }

    #[test]
    fn test_extract_sessions() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "allComposers": [
                {
                    "composerId": "abc-123",
                    "name": "Fix auth bug",
                    "unifiedMode": "agent",
                    "createdAt": 1700000000000,
                    "lastUpdatedAt": 1700001000000
                },
                {
                    "composerId": "def-456",
                    "name": "",
                    "unifiedMode": "chat",
                    "createdAt": 1699999000000
                }
            ]
        }"#;

        create_test_db(tmp.path(), json);

        let sessions = extract_sessions(tmp.path(), "/tmp/my-project");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "abc-123");
        assert_eq!(sessions[0].name, "Fix auth bug");
        assert_eq!(sessions[0].mode, "agent");
        assert_eq!(sessions[0].updated_at, Some(1700001000000));
        assert_eq!(sessions[1].session_id, "def-456");
        assert!(sessions[1].name.is_empty());
    }

    #[test]
    fn test_extract_sessions_no_db() {
        let tmp = TempDir::new().unwrap();
        let sessions = extract_sessions(tmp.path(), "/tmp");
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_extract_sessions_empty_composers() {
        let tmp = TempDir::new().unwrap();
        create_test_db(tmp.path(), r#"{"allComposers": []}"#);
        let sessions = extract_sessions(tmp.path(), "/tmp");
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_extract_sessions_skip_empty_ids() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "allComposers": [
                { "composerId": "", "name": "no id" },
                { "composerId": "valid-id", "name": "has id" }
            ]
        }"#;
        create_test_db(tmp.path(), json);
        let sessions = extract_sessions(tmp.path(), "/tmp");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "valid-id");
    }

    #[test]
    fn test_extract_subagent_info() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "allComposers": [
                {
                    "composerId": "parent-uuid",
                    "name": "Main session",
                    "unifiedMode": "agent",
                    "createdAt": 1700000000000
                },
                {
                    "composerId": "child-uuid",
                    "name": "Subtask",
                    "unifiedMode": "agent",
                    "createdAt": 1700000010000,
                    "subagentInfo": {
                        "parentComposerId": "parent-uuid",
                        "subagentType": 2
                    }
                }
            ]
        }"#;
        create_test_db(tmp.path(), json);

        let sessions = extract_sessions(tmp.path(), "/tmp");
        assert_eq!(sessions.len(), 2);

        let parent = sessions
            .iter()
            .find(|s| s.session_id == "parent-uuid")
            .unwrap();
        assert!(parent.parent_session_id.is_none());
        assert!(parent.subagent_type.is_none());
        assert!(!parent.is_subagent());

        let child = sessions
            .iter()
            .find(|s| s.session_id == "child-uuid")
            .unwrap();
        assert_eq!(child.parent_session_id.as_deref(), Some("parent-uuid"));
        assert_eq!(child.subagent_type.as_deref(), Some("explore"));
        assert!(child.is_subagent());
    }

    #[test]
    fn test_map_subagent_type() {
        assert_eq!(map_subagent_type(0), "generalPurpose");
        assert_eq!(map_subagent_type(1), "shell");
        assert_eq!(map_subagent_type(2), "explore");
        assert_eq!(map_subagent_type(3), "explore");
        assert_eq!(map_subagent_type(4), "browser-use");
        assert_eq!(map_subagent_type(5), "best-of-n-runner");
        assert_eq!(map_subagent_type(99), "unknown-99");
    }
}
