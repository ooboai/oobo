use std::path::Path;

use super::Session;

const COMPOSER_KEY: &str = "composer.composerData";

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
}
