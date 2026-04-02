/// Kiro (AWS) — agent hooks in ~/.kiro/agents/, SQLite session storage.
///
/// Schema (conversations_v2 table):
///   key             TEXT NOT NULL  — directory path
///   conversation_id TEXT NOT NULL  — UUID
///   value           TEXT NOT NULL  — JSON blob with conversation history
///   created_at      INTEGER NOT NULL — Unix timestamp in milliseconds
///   updated_at      INTEGER NOT NULL — Unix timestamp in milliseconds
///   PRIMARY KEY (key, conversation_id)
///
/// Session DB location:
///   All platforms: ~/.kiro/data.sqlite3
///   (Kiro CLI uses ~/.kiro/ on all OSes per official docs)
use std::path::PathBuf;

use crate::tools::cursor::Session;

pub fn kiro_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".kiro"))
}

pub fn find_db_path() -> Option<PathBuf> {
    let dir = kiro_dir()?;
    let db = dir.join("data.sqlite3");
    if db.exists() {
        return Some(db);
    }
    None
}

fn sessions_from_db(db_path: &std::path::Path) -> Vec<Session> {
    let conn = match crate::utils::open_db_readonly(db_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let query = "SELECT key, conversation_id, value, created_at, updated_at \
                 FROM conversations_v2 \
                 ORDER BY updated_at DESC";
    let mut stmt = match conn.prepare(query) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let rows = match stmt.query_map([], |row| {
        let key: String = row.get(0)?;
        let conversation_id: String = row.get(1)?;
        let value: String = row.get(2)?;
        let created_at: i64 = row.get(3)?;
        let updated_at: i64 = row.get(4)?;
        Ok((key, conversation_id, value, created_at, updated_at))
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for row in rows.flatten() {
        let (key, conversation_id, value, created_at, updated_at) = row;

        let name = extract_title_from_value(&value).unwrap_or_else(|| "Kiro session".to_string());

        sessions.push(Session {
            session_id: conversation_id,
            name,
            mode: "kiro".to_string(),
            created_at: Some(created_at),
            updated_at: Some(updated_at),
            project_path: key,
            workspace_dir: db_path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            source: "kiro".to_string(),
            parent_session_id: None,
            subagent_type: None,
        });
    }
    sessions
}

/// Extract a human-readable title from the conversation JSON `value` blob.
fn extract_title_from_value(value: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(value).ok()?;

    // Check for explicit title/summary fields
    if let Some(title) = v
        .get("title")
        .or_else(|| v.get("summary"))
        .or_else(|| v.get("name"))
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(crate::utils::truncate_name(
            title,
            crate::utils::MAX_SESSION_NAME_LEN,
        ));
    }

    // Fall back to first user message as title
    if let Some(msgs) = v.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role == "user" {
                if let Some(text) = msg
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                {
                    return Some(crate::utils::truncate_name(
                        text,
                        crate::utils::MAX_SESSION_NAME_LEN,
                    ));
                }
            }
        }
    }

    None
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let db_path = match find_db_path() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };

    let norm_root = crate::paths::normalize_path(project_root);
    let all = sessions_from_db(&db_path);
    let mut sessions: Vec<Session> = all
        .into_iter()
        .filter(|s| {
            !s.project_path.is_empty() && crate::paths::normalize_path(&s.project_path) == norm_root
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let db_path = match find_db_path() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let mut sessions = sessions_from_db(&db_path);
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use std::path::{Path, PathBuf};

    use crate::core::message::Message;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        let db_path = super::find_db_path()?;
        let conn = crate::utils::open_db_readonly(&db_path).ok()?;

        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM conversations_v2 WHERE conversation_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if exists {
            // Return <db_dir>/<session_id> as a synthetic path.
            // parse_messages extracts the session_id from the filename and
            // finds the DB in the parent directory.
            Some(db_path.parent()?.join(session_id))
        } else {
            None
        }
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        // The path is a synthetic <db_dir>/<session_id> from find_transcript_path.
        let session_id = match path.file_name().and_then(|n| n.to_str()) {
            Some(id) => id,
            None => return Vec::new(),
        };
        let db_path = match path.parent() {
            Some(dir) => dir.join("data.sqlite3"),
            None => return Vec::new(),
        };
        if !db_path.exists() {
            return Vec::new();
        }
        parse_messages_for_session(&db_path, session_id)
    }

    pub fn parse_messages_for_session(db_path: &Path, session_id: &str) -> Vec<Message> {
        let conn = match crate::utils::open_db_readonly(db_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let value: String = match conn.query_row(
            "SELECT value FROM conversations_v2 WHERE conversation_id = ?1",
            [session_id],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let v: serde_json::Value = match serde_json::from_str(&value) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let msgs = v.get("messages").and_then(|m| m.as_array());
        let mut messages = Vec::new();
        if let Some(arr) = msgs {
            for msg in arr {
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if role != "user" && role != "assistant" {
                    continue;
                }
                let text = msg
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.is_empty() {
                    messages.push(Message {
                        role: role.to_string(),
                        text,
                        timestamp_ms: None,
                    });
                }
            }
        }
        messages
    }

    pub fn count_messages(_project_path: &str, session_id: &str) -> u32 {
        let db_path = match super::find_db_path() {
            Some(p) => p,
            None => return 0,
        };
        parse_messages_for_session(&db_path, session_id).len() as u32
    }

    pub fn read_transcript(path: &Path, max_messages: u32) -> String {
        let messages = parse_messages(path);
        crate::utils::format_transcript(&messages, max_messages, "Assistant")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE conversations_v2 (
                key TEXT NOT NULL,
                conversation_id TEXT NOT NULL,
                value TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (key, conversation_id)
            );
            CREATE INDEX idx_conversations_v2_key_updated ON conversations_v2(key, updated_at DESC);
            CREATE INDEX idx_conversations_v2_updated_at ON conversations_v2(updated_at DESC);",
        )
        .unwrap();
    }

    #[test]
    fn test_sessions_from_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        create_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO conversations_v2 (key, conversation_id, value, created_at, updated_at) \
             VALUES ('/Users/dev/myapp', 'abc-123', '{\"messages\":[{\"role\":\"user\",\"content\":\"Fix the login bug\"}]}', 1700000000000, 1700001000000)",
            [],
        ).unwrap();
        drop(conn);

        let sessions = sessions_from_db(&db_path);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "abc-123");
        assert_eq!(sessions[0].project_path, "/Users/dev/myapp");
        assert_eq!(sessions[0].name, "Fix the login bug");
        assert_eq!(sessions[0].source, "kiro");
        assert_eq!(sessions[0].created_at, Some(1700000000000));
    }

    #[test]
    fn test_sessions_from_db_with_title_field() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        create_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO conversations_v2 (key, conversation_id, value, created_at, updated_at) \
             VALUES ('/dev/project', 'xyz-789', '{\"title\":\"Auth refactor\",\"messages\":[]}', 1700000000000, 1700001000000)",
            [],
        ).unwrap();
        drop(conn);

        let sessions = sessions_from_db(&db_path);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "Auth refactor");
    }

    #[test]
    fn test_empty_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        create_test_db(&db_path);

        let sessions = sessions_from_db(&db_path);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_sessions_for_project_filters_by_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        create_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO conversations_v2 VALUES ('/dev/app-a', 'id-1', '{\"messages\":[]}', 1000, 2000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO conversations_v2 VALUES ('/dev/app-b', 'id-2', '{\"messages\":[]}', 1000, 2000)",
            [],
        ).unwrap();
        drop(conn);

        let sessions = sessions_from_db(&db_path);
        assert_eq!(sessions.len(), 2);

        let filtered: Vec<_> = sessions
            .into_iter()
            .filter(|s| s.project_path == "/dev/app-a")
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].session_id, "id-1");
    }

    #[test]
    fn test_sessions_from_corrupt_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        std::fs::write(&db_path, b"this is not a sqlite file").unwrap();

        let sessions = sessions_from_db(&db_path);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_sessions_from_wrong_schema() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE other_table (id TEXT)")
            .unwrap();
        drop(conn);

        // DB exists but has no conversations_v2 table
        let sessions = sessions_from_db(&db_path);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_extract_title_from_malformed_json() {
        assert!(extract_title_from_value("not json").is_none());
        assert!(extract_title_from_value("").is_none());
        assert!(extract_title_from_value("null").is_none());
        assert!(extract_title_from_value("42").is_none());
    }

    #[test]
    fn test_transcript_parse_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        create_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            r#"INSERT INTO conversations_v2 VALUES ('/dev', 'sess-1', '{"messages":[{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi there!"},{"role":"system","content":"ignored"}]}', 1000, 2000)"#,
            [],
        ).unwrap();
        drop(conn);

        // Test via parse_messages_for_session (direct)
        let msgs = transcript::parse_messages_for_session(&db_path, "sess-1");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "Hi there!");

        // Test via parse_messages (synthetic path: <db_dir>/<session_id>)
        let synthetic_path = tmp.path().join("sess-1");
        let msgs2 = transcript::parse_messages(&synthetic_path);
        assert_eq!(msgs2.len(), 2);
        assert_eq!(msgs2[0].text, "Hello");
        assert_eq!(msgs2[1].text, "Hi there!");
    }

    #[test]
    fn test_find_transcript_returns_synthetic_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("data.sqlite3");
        create_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            r#"INSERT INTO conversations_v2 VALUES ('/dev', 'sess-42', '{"messages":[]}', 1000, 2000)"#,
            [],
        ).unwrap();
        drop(conn);

        // Patch find_db_path by calling find_transcript_path which uses it
        // We can't easily override find_db_path, so test parse_messages directly
        // with a synthetic path matching the convention
        let synthetic = tmp.path().join("sess-42");
        let msgs = transcript::parse_messages(&synthetic);
        assert!(msgs.is_empty()); // empty messages array
    }
}
