use std::path::{Path, PathBuf};

use crate::tools::cursor::Session;

mod legacy;
pub mod transcript;

pub(crate) use legacy::{sessions_from_legacy_db, stats_from_legacy_db};

fn opencode_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/opencode"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::data_dir().map(|d| d.join("opencode"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir().map(|d| d.join("opencode"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

fn xdg_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("opencode"))
}

pub fn find_db_path() -> Option<PathBuf> {
    for dir_fn in [opencode_data_dir, xdg_data_dir] {
        if let Some(dir) = dir_fn() {
            let db = dir.join("opencode.db");
            if db.exists() {
                return Some(db);
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        let db = home.join(".local/share/opencode/opencode.db");
        if db.exists() {
            return Some(db);
        }
    }

    None
}

pub(crate) enum DbSchema {
    /// Old schema: session table has prompt_tokens, completion_tokens, cost columns.
    Legacy,
    /// New schema (v1.2+): tokens live in message.data JSON, projects in separate table.
    Modern,
}

pub(crate) fn detect_schema(conn: &rusqlite::Connection) -> DbSchema {
    let has_project_table: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='project'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if has_project_table {
        DbSchema::Modern
    } else {
        DbSchema::Legacy
    }
}

// Modern schema (v1.2+)

fn sessions_from_modern_db(conn: &rusqlite::Connection, db_path: &Path) -> Vec<Session> {
    let has_parent_col = conn
        .prepare("SELECT parent_id FROM session LIMIT 0")
        .is_ok();

    let query = if has_parent_col {
        "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated, p.worktree, \
                s.parent_id \
         FROM session s \
         LEFT JOIN project p ON s.project_id = p.id \
         WHERE s.time_archived IS NULL \
         ORDER BY s.time_updated DESC"
    } else {
        "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated, p.worktree \
         FROM session s \
         LEFT JOIN project p ON s.project_id = p.id \
         WHERE s.time_archived IS NULL \
         ORDER BY s.time_updated DESC"
    };

    let mut stmt = match conn.prepare(query) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let rows = match stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let directory: String = row.get(2)?;
        let time_created: i64 = row.get(3)?;
        let time_updated: i64 = row.get(4)?;
        let worktree: Option<String> = row.get(5)?;
        let parent_session_id: Option<String> = if has_parent_col {
            row.get(6).ok().flatten()
        } else {
            None
        };
        Ok((
            id,
            title,
            directory,
            time_created,
            time_updated,
            worktree,
            parent_session_id,
        ))
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for row in rows.flatten() {
        let (id, title, directory, time_created, time_updated, worktree, parent_session_id) = row;

        let project_path = if !directory.is_empty() && directory != "/" {
            directory
        } else {
            worktree.unwrap_or_default()
        };

        let name = if title.is_empty() {
            "OpenCode session".to_string()
        } else {
            crate::utils::truncate_name(&title, crate::utils::MAX_SESSION_NAME_LEN)
        };

        let created_ms = normalize_ts(time_created);
        let updated_ms = normalize_ts(time_updated);

        sessions.push(Session {
            session_id: id,
            name,
            mode: "opencode".to_string(),
            created_at: Some(created_ms),
            updated_at: Some(updated_ms),
            project_path,
            workspace_dir: db_path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            source: "opencode".to_string(),
            parent_session_id,
            subagent_type: None,
        });
    }

    sessions
}

/// Aggregate per-message native token data from message.data JSON.
pub(crate) fn stats_from_modern_db(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Option<crate::remote::payload::SessionStats> {
    let mut stmt = conn
        .prepare(
            "SELECT data, time_created FROM message \
             WHERE session_id = ?1 ORDER BY time_created ASC",
        )
        .ok()?;

    let rows = stmt
        .query_map([session_id], |row| {
            let data: String = row.get(0)?;
            let ts: i64 = row.get(1)?;
            Ok((data, ts))
        })
        .ok()?;

    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_read: u64 = 0;
    let mut cache_write: u64 = 0;
    let mut model: Option<String> = None;
    let mut tool_call_count: u32 = 0;
    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;

    for row in rows.flatten() {
        let (data_str, ts) = row;
        if first_ts.is_none() {
            first_ts = Some(ts);
        }
        last_ts = Some(ts);

        let v: serde_json::Value = match serde_json::from_str(&data_str) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role != "assistant" {
            continue;
        }

        if let Some(tokens) = v.get("tokens") {
            input_tokens += tokens.get("input").and_then(serde_json::Value::as_u64).unwrap_or(0);
            output_tokens += tokens.get("output").and_then(serde_json::Value::as_u64).unwrap_or(0);
            if let Some(cache) = tokens.get("cache") {
                cache_read += cache.get("read").and_then(serde_json::Value::as_u64).unwrap_or(0);
                cache_write += cache.get("write").and_then(serde_json::Value::as_u64).unwrap_or(0);
            }
        }

        if model.is_none() {
            model = v
                .get("modelID")
                .and_then(|m| m.as_str())
                .map(std::string::ToString::to_string);
        }

        let finish = v.get("finish").and_then(|f| f.as_str()).unwrap_or("");
        if finish == "tool-calls" {
            tool_call_count += 1;
        }
    }

    let files_touched = extract_files_touched(conn, session_id);

    let duration_secs = match (first_ts, last_ts) {
        (Some(f), Some(l)) if l > f => {
            let f_ms = normalize_ts(f);
            let l_ms = normalize_ts(l);
            let secs = (l_ms - f_ms) / 1000;
            if secs > 0 {
                Some(secs as u64)
            } else {
                None
            }
        }
        _ => None,
    };

    let has_tokens = input_tokens > 0 || output_tokens > 0;

    Some(crate::remote::payload::SessionStats {
        model,
        input_tokens: if has_tokens { Some(input_tokens) } else { None },
        output_tokens: if has_tokens {
            Some(output_tokens)
        } else {
            None
        },
        duration_secs,
        files_touched,
        tool_call_count,
        cache_read_tokens: if cache_read > 0 {
            Some(cache_read)
        } else {
            None
        },
        cache_creation_tokens: if cache_write > 0 {
            Some(cache_write)
        } else {
            None
        },
        is_estimated: false,
        token_source: if has_tokens {
            Some("native".to_string())
        } else {
            None
        },
    })
}

/// Extract file paths from tool call parts for a session.
fn extract_files_touched(conn: &rusqlite::Connection, session_id: &str) -> Vec<String> {
    let mut stmt = match conn
        .prepare("SELECT data FROM part WHERE session_id = ?1 ORDER BY time_created ASC")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let rows = match stmt.query_map([session_id], |row| {
        let data: String = row.get(0)?;
        Ok(data)
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut files = Vec::new();
    for row in rows.flatten() {
        let v: serde_json::Value = match serde_json::from_str(&row) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let part_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if part_type == "tool" {
            if let Some(state) = v.get("state") {
                if let Some(input) = state.get("input") {
                    for key in ["path", "file_path", "file", "filePath", "pattern"] {
                        if let Some(fp) = input.get(key).and_then(|v| v.as_str()) {
                            let f = fp.to_string();
                            if !files.contains(&f) {
                                files.push(f);
                            }
                        }
                    }
                }
            }
        }
    }
    files
}

// Helpers

pub(crate) fn normalize_ts(ts: i64) -> i64 {
    if ts > 1_000_000_000_000 {
        ts
    } else {
        ts * 1000
    }
}

fn try_resolve_project_path(db_path: &Path, session_id: &str) -> Option<String> {
    let conn = crate::utils::open_db_readonly(db_path).ok()?;

    match detect_schema(&conn) {
        DbSchema::Modern => conn
            .query_row(
                "SELECT COALESCE(NULLIF(s.directory, '/'), p.worktree) \
                 FROM session s LEFT JOIN project p ON s.project_id = p.id \
                 WHERE s.id = ?1",
                [session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .filter(|p| !p.is_empty() && p != "/"),
        DbSchema::Legacy => {
            let cwd: Option<String> = conn
                .query_row(
                    "SELECT content FROM message WHERE session_id = ?1 AND role = 'system' \
                     ORDER BY created_at ASC LIMIT 1",
                    [session_id],
                    |row| row.get(0),
                )
                .ok();

            if let Some(content) = cwd {
                if content.contains("cwd") || content.contains("working directory") {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with('/') && !trimmed.contains(' ') {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
            None
        }
    }
}

// Public API

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let db_path = match find_db_path() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };

    let norm_root = crate::paths::normalize_path(project_root);

    let conn = crate::utils::open_db_readonly(&db_path).map_err(|e| format!("opencode db: {e}"))?;

    let mut sessions: Vec<Session> = match detect_schema(&conn) {
        DbSchema::Modern => sessions_from_modern_db(&conn, &db_path),
        DbSchema::Legacy => {
            let all = sessions_from_legacy_db(&conn, &db_path);
            all.into_iter()
                .map(|(mut s, _)| {
                    if s.project_path.is_empty() {
                        s.project_path =
                            try_resolve_project_path(&db_path, &s.session_id).unwrap_or_default();
                    }
                    s
                })
                .collect()
        }
    };

    sessions.retain(|s| {
        !s.project_path.is_empty() && crate::paths::normalize_path(&s.project_path) == norm_root
    });
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let db_path = match find_db_path() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };

    let conn = crate::utils::open_db_readonly(&db_path).map_err(|e| format!("opencode db: {e}"))?;

    let mut sessions = match detect_schema(&conn) {
        DbSchema::Modern => sessions_from_modern_db(&conn, &db_path),
        DbSchema::Legacy => sessions_from_legacy_db(&conn, &db_path)
            .into_iter()
            .map(|(s, _)| s)
            .collect(),
    };

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_legacy_test_db(path: &Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                message_count INTEGER NOT NULL DEFAULT 0,
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                cost REAL NOT NULL DEFAULT 0.0,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                parent_session_id TEXT,
                summary TEXT
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (session_id) REFERENCES session(id)
            );",
        )
        .unwrap();
    }

    fn create_modern_test_db(path: &Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (
                id TEXT PRIMARY KEY,
                worktree TEXT NOT NULL,
                vcs TEXT,
                name TEXT,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                sandboxes TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                parent_id TEXT,
                slug TEXT NOT NULL DEFAULT '',
                directory TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                version TEXT NOT NULL DEFAULT '',
                time_created INTEGER NOT NULL DEFAULT 0,
                time_updated INTEGER NOT NULL DEFAULT 0,
                time_archived INTEGER,
                FOREIGN KEY (project_id) REFERENCES project(id) ON DELETE CASCADE
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL,
                FOREIGN KEY (message_id) REFERENCES message(id) ON DELETE CASCADE
            );",
        )
        .unwrap();
    }

    #[test]
    fn test_legacy_sessions_from_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_legacy_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO session (id, title, message_count, prompt_tokens, completion_tokens, cost, created_at, updated_at) \
             VALUES ('sess-1', 'Fix login bug', 10, 5000, 3000, 0.15, 1700000000000, 1700000120000)",
            [],
        ).unwrap();

        let sessions = sessions_from_legacy_db(&conn, &db_path);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0.session_id, "sess-1");
        assert_eq!(sessions[0].0.name, "Fix login bug");
        assert_eq!(sessions[0].0.source, "opencode");
        assert_eq!(sessions[0].1.prompt_tokens, 5000);
        assert_eq!(sessions[0].1.completion_tokens, 3000);
    }

    #[test]
    fn test_modern_sessions_from_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_modern_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated) VALUES ('proj1', '/Users/dev/myapp', 1000, 1000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, time_created, time_updated) \
             VALUES ('ses1', 'proj1', '/Users/dev/myapp', 'Fix auth', 1772701435137, 1772701472441)",
            [],
        ).unwrap();

        let sessions = sessions_from_modern_db(&conn, &db_path);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "ses1");
        assert_eq!(sessions[0].name, "Fix auth");
        assert_eq!(sessions[0].project_path, "/Users/dev/myapp");
        assert!(sessions[0].parent_session_id.is_none());
    }

    #[test]
    fn test_modern_parent_id_propagation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_modern_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated) VALUES ('p1', '/dev/app', 1000, 1000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, title, time_created, time_updated) \
             VALUES ('parent-1', 'p1', 'Main session', 1772701435000, 1772701472000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, parent_id, title, time_created, time_updated) \
             VALUES ('child-1', 'p1', 'parent-1', 'Subagent task', 1772701440000, 1772701460000)",
            [],
        )
        .unwrap();

        let sessions = sessions_from_modern_db(&conn, &db_path);
        assert_eq!(sessions.len(), 2);

        let parent = sessions
            .iter()
            .find(|s| s.session_id == "parent-1")
            .unwrap();
        assert!(parent.parent_session_id.is_none());
        assert!(!parent.is_subagent());

        let child = sessions.iter().find(|s| s.session_id == "child-1").unwrap();
        assert_eq!(child.parent_session_id.as_deref(), Some("parent-1"));
        assert!(child.is_subagent());
    }

    #[test]
    fn test_modern_native_stats() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_modern_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated) VALUES ('p1', '/dev', 1000, 1000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, title, time_created, time_updated) \
             VALUES ('s1', 'p1', 'test', 1772701435000, 1772701472000)",
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES
            ('m1', 's1', 1772701435000, 1772701435000, '{"role":"user","time":{"created":1772701435000}}')"#,
            [],
        ).unwrap();
        conn.execute(
            r#"INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES
            ('m2', 's1', 1772701446000, 1772701446000, '{"role":"assistant","modelID":"big-pickle","cost":0.05,"tokens":{"total":11456,"input":78,"output":89,"reasoning":0,"cache":{"read":510,"write":10779}},"finish":"tool-calls"}')"#,
            [],
        ).unwrap();
        conn.execute(
            r#"INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES
            ('m3', 's1', 1772701453000, 1772701453000, '{"role":"assistant","modelID":"big-pickle","cost":0.03,"tokens":{"total":12105,"input":483,"output":142,"reasoning":0,"cache":{"read":11289,"write":191}},"finish":"stop"}')"#,
            [],
        ).unwrap();

        let stats = stats_from_modern_db(&conn, "s1").unwrap();
        assert_eq!(stats.model, Some("big-pickle".to_string()));
        assert_eq!(stats.input_tokens, Some(561));
        assert_eq!(stats.output_tokens, Some(231));
        assert_eq!(stats.cache_read_tokens, Some(11799));
        assert_eq!(stats.cache_creation_tokens, Some(10970));
        assert_eq!(stats.tool_call_count, 1);
        assert_eq!(stats.token_source, Some("native".to_string()));
    }

    #[test]
    fn test_legacy_extract_stats() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_legacy_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO session (id, title, message_count, prompt_tokens, completion_tokens, cost, created_at, updated_at) \
             VALUES ('s1', 'test', 10, 15000, 8000, 0.45, 1700000000000, 1700000120000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, role, content, created_at) VALUES ('m1', 's1', 'tool', 'result', 1000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, role, content, created_at) VALUES ('m2', 's1', 'tool', 'result2', 2000)",
            [],
        ).unwrap();

        let stats = stats_from_legacy_db(&conn, "s1").unwrap();
        assert_eq!(stats.input_tokens, Some(15000));
        assert_eq!(stats.output_tokens, Some(8000));
        assert_eq!(stats.tool_call_count, 2);
        assert_eq!(stats.duration_secs, Some(120));
    }

    #[test]
    fn test_schema_detection() {
        let tmp = tempfile::TempDir::new().unwrap();

        let legacy_path = tmp.path().join("legacy.db");
        create_legacy_test_db(&legacy_path);
        let conn = rusqlite::Connection::open(&legacy_path).unwrap();
        assert!(matches!(detect_schema(&conn), DbSchema::Legacy));

        let modern_path = tmp.path().join("modern.db");
        create_modern_test_db(&modern_path);
        let conn = rusqlite::Connection::open(&modern_path).unwrap();
        assert!(matches!(detect_schema(&conn), DbSchema::Modern));
    }

    #[test]
    fn test_empty_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_legacy_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let sessions = sessions_from_legacy_db(&conn, &db_path);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_modern_parse_messages_for_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_modern_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated) VALUES ('p1', '/dev', 1000, 1000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, title, time_created, time_updated) \
             VALUES ('s1', 'p1', 'test session', 1772701435000, 1772701472000)",
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES
            ('m1', 's1', 1772701435000, 1772701435000, '{"role":"user"}')"#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES
            ('m2', 's1', 1772701446000, 1772701446000, '{"role":"assistant","modelID":"gpt-4"}')"#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES
            ('p1', 'm1', 's1', 1772701435000, 1772701435000, '{"type":"text","text":"Hello"}')"#,
            [],
        ).unwrap();
        conn.execute(
            r#"INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES
            ('p2', 'm2', 's1', 1772701446000, 1772701446000, '{"type":"text","text":"Hi there!"}')"#,
            [],
        ).unwrap();
        drop(conn);

        let messages = transcript::parse_messages_for_session(&db_path, "s1");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "Hello");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].text, "Hi there!");
    }

    #[test]
    fn test_read_transcript_for_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_legacy_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO session (id, title, created_at, updated_at) VALUES ('s1', 'test', 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, role, content, created_at) VALUES ('m1', 's1', 'user', 'hello', 1000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, role, content, created_at) VALUES ('m2', 's1', 'assistant', 'hi there', 2000)",
            [],
        ).unwrap();
        drop(conn);

        let text = transcript::read_transcript_for_session(&db_path, "s1", 10);
        assert!(!text.is_empty());
        assert!(text.contains("hello"));
        assert!(text.contains("hi there"));
    }

    #[test]
    fn test_parse_messages_returns_empty_without_session_context() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_legacy_test_db(&db_path);

        let messages = transcript::parse_messages(&db_path);
        assert!(
            messages.is_empty(),
            "parse_messages without session context should return empty"
        );

        let text = transcript::read_transcript(&db_path, 10);
        assert!(
            text.is_empty(),
            "read_transcript without session context should return empty"
        );
    }

    #[test]
    fn test_title_truncation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_legacy_test_db(&db_path);

        let long_title = "a".repeat(100);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO session (id, title, created_at, updated_at) VALUES ('s1', ?1, 0, 0)",
            [&long_title],
        )
        .unwrap();

        let sessions = sessions_from_legacy_db(&conn, &db_path);
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].0.name.len() <= 64);
        assert!(sessions[0].0.name.ends_with('…'));
    }
}
