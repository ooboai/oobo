use std::path::Path;

use super::Session;

const COMPOSER_KEY: &str = "composer.composerData";

/// Map Cursor's numeric subagent type IDs to human-readable names.
/// These values come from Cursor's internal `SubagentType` enum in
/// composer data (observed via state.vscdb inspection). IDs 2 and 3
/// are both explore variants (quick/thorough) — collapsed to "explore".
fn map_subagent_type(type_id: u64) -> String {
    match type_id {
        0 => "generalPurpose".to_string(),
        1 => "shell".to_string(),
        2 => "explore".to_string(),
        3 => "explore".to_string(),
        4 => "browser-use".to_string(),
        5 => "best-of-n-runner".to_string(),
        _ => format!("unknown-{type_id}"),
    }
}

/// Extract sessions from a workspace's state.vscdb.
pub fn extract_sessions(ws_dir: &Path, project_path: &str) -> Vec<Session> {
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
        Some(arr) => arr,
        None => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for c in composers {
        let cid = match c.get("composerId").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };

        let (parent_id, subagent_tp) = c
            .get("subagentInfo")
            .map(|info| {
                let parent = info
                    .get("parentComposerId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let stype = info
                    .get("subagentType")
                    .and_then(|v| v.as_u64())
                    .map(map_subagent_type);
                (parent, stype)
            })
            .unwrap_or((None, None));

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
            source: "composer".to_string(),
            parent_session_id: parent_id,
            subagent_type: subagent_tp,
        });
    }

    sessions
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
