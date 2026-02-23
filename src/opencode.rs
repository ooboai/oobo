use std::fs;
use std::path::{Path, PathBuf};

use crate::cursor::transcript::Message;
use crate::cursor::Session;

fn opencode_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/opencode"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::data_dir().map(|d| d.join("opencode"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn xdg_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("opencode"))
}

fn find_db_path() -> Option<PathBuf> {
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

fn sessions_from_db(db_path: &Path) -> Vec<(Session, SessionMeta)> {
    let conn = match rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut stmt = match conn.prepare(
        "SELECT id, title, message_count, prompt_tokens, completion_tokens, \
         cost, created_at, updated_at, parent_session_id, summary \
         FROM session ORDER BY updated_at DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let message_count: i64 = row.get(2)?;
        let prompt_tokens: i64 = row.get(3)?;
        let completion_tokens: i64 = row.get(4)?;
        let cost: f64 = row.get(5)?;
        let created_at: i64 = row.get(6)?;
        let updated_at: i64 = row.get(7)?;
        let parent_session_id: Option<String> = row.get(8)?;
        let summary: Option<String> = row.get(9)?;
        Ok((
            id,
            title,
            message_count,
            prompt_tokens,
            completion_tokens,
            cost,
            created_at,
            updated_at,
            parent_session_id,
            summary,
        ))
    });

    let rows = match rows {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for row in rows.flatten() {
        let (
            id,
            title,
            message_count,
            prompt_tokens,
            completion_tokens,
            cost,
            created_at,
            updated_at,
            _parent_session_id,
            _summary,
        ) = row;

        let name = if title.is_empty() {
            "OpenCode session".to_string()
        } else if title.len() > 60 {
            format!("{}...", &title[..57])
        } else {
            title
        };

        let created_ms = if created_at > 1_000_000_000_000 {
            created_at
        } else {
            created_at * 1000
        };
        let updated_ms = if updated_at > 1_000_000_000_000 {
            updated_at
        } else {
            updated_at * 1000
        };

        let meta = SessionMeta {
            message_count: message_count as u32,
            prompt_tokens: prompt_tokens as u64,
            completion_tokens: completion_tokens as u64,
            cost,
        };

        sessions.push((
            Session {
                session_id: id,
                name,
                mode: "opencode".to_string(),
                created_at: Some(created_ms),
                updated_at: Some(updated_ms),
                project_path: String::new(),
                workspace_dir: db_path
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
                source: "opencode".to_string(),
            },
            meta,
        ));
    }

    sessions
}

#[allow(dead_code)]
struct SessionMeta {
    message_count: u32,
    prompt_tokens: u64,
    completion_tokens: u64,
    cost: f64,
}

fn try_resolve_project_path(db_path: &Path, session_id: &str) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;

    let cwd: Option<String> = conn
        .query_row(
            "SELECT content FROM message WHERE session_id = ?1 AND role = 'system' ORDER BY created_at ASC LIMIT 1",
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

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let db_path = match find_db_path() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };

    let norm_root = normalize(project_root);
    let all = sessions_from_db(&db_path);
    let mut sessions: Vec<Session> = all
        .into_iter()
        .map(|(mut s, _)| {
            if s.project_path.is_empty() {
                s.project_path =
                    try_resolve_project_path(&db_path, &s.session_id).unwrap_or_default();
            }
            s
        })
        .filter(|s| !s.project_path.is_empty() && normalize(&s.project_path) == norm_root)
        .collect();

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let db_path = match find_db_path() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };

    let all = sessions_from_db(&db_path);
    let mut sessions: Vec<Session> = all.into_iter().map(|(s, _)| s).collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

fn normalize(p: &str) -> String {
    match fs::canonicalize(p) {
        Ok(c) => c.to_string_lossy().to_string(),
        Err(_) => p.trim_end_matches('/').to_string(),
    }
}

pub mod transcript {
    use super::*;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        let db_path = find_db_path()?;
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;

        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM message WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if exists {
            Some(db_path)
        } else {
            None
        }
    }

    pub fn count_messages(_project_path: &str, session_id: &str) -> u32 {
        let db_path = match find_db_path() {
            Some(p) => p,
            None => return 0,
        };

        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => return 0,
        };

        conn.query_row(
            "SELECT COUNT(*) FROM message WHERE session_id = ?1 AND role IN ('user', 'assistant')",
            [session_id],
            |row| row.get::<_, u32>(0),
        )
        .unwrap_or(0)
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let session_id = match extract_session_id_from_context(path) {
            Some(id) => id,
            None => return Vec::new(),
        };
        parse_messages_for_session(path, &session_id)
    }

    pub fn parse_messages_for_session(db_path: &Path, session_id: &str) -> Vec<Message> {
        let conn = match rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut stmt = match conn.prepare(
            "SELECT role, content FROM message \
             WHERE session_id = ?1 AND role IN ('user', 'assistant') \
             ORDER BY created_at ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map([session_id], |row| {
            let role: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((role, content))
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut messages = Vec::new();
        for row in rows.flatten() {
            let (role, content) = row;
            let text = extract_text_content(&content);
            if !text.is_empty() {
                messages.push(Message { role, text });
            }
        }
        messages
    }

    fn extract_text_content(content: &str) -> String {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(arr) = v.as_array() {
                let texts: Vec<&str> = arr
                    .iter()
                    .filter_map(|part| {
                        let t = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if t == "text" {
                            part.get("text").and_then(|v| v.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !texts.is_empty() {
                    return texts.join("\n");
                }
            }
            if let Some(s) = v.as_str() {
                return s.to_string();
            }
        }
        content.to_string()
    }

    fn extract_session_id_from_context(_db_path: &Path) -> Option<String> {
        None
    }

    pub fn extract_stats(
        db_path: &Path,
        session_id: &str,
    ) -> Option<crate::server::payload::SessionStats> {
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;

        let row: (i64, i64, f64, i64, i64) = conn
            .query_row(
                "SELECT prompt_tokens, completion_tokens, cost, created_at, updated_at \
                 FROM session WHERE id = ?1",
                [session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .ok()?;

        let (prompt_tokens, completion_tokens, cost, created_at, updated_at) = row;

        let created_ms = if created_at > 1_000_000_000_000 {
            created_at
        } else {
            created_at * 1000
        };
        let updated_ms = if updated_at > 1_000_000_000_000 {
            updated_at
        } else {
            updated_at * 1000
        };
        let duration_secs = if updated_ms > created_ms {
            let secs = (updated_ms - created_ms) / 1000;
            if secs > 0 {
                Some(secs as u64)
            } else {
                None
            }
        } else {
            None
        };

        let tool_call_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM message \
                 WHERE session_id = ?1 AND role = 'tool'",
                [session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let mut model: Option<String> = None;
        if let Ok(content) = conn.query_row::<String, _, _>(
            "SELECT content FROM message WHERE session_id = ?1 AND role = 'assistant' \
             ORDER BY created_at ASC LIMIT 1",
            [session_id],
            |row| row.get(0),
        ) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(m) = v.get("model").and_then(|v| v.as_str()) {
                    model = Some(m.to_string());
                }
            }
        }

        Some(crate::server::payload::SessionStats {
            model,
            input_tokens: if prompt_tokens > 0 {
                Some(prompt_tokens as u64)
            } else {
                None
            },
            output_tokens: if completion_tokens > 0 {
                Some(completion_tokens as u64)
            } else {
                None
            },
            total_cost_usd: if cost > 0.0 { Some(cost) } else { None },
            duration_secs,
            files_touched: Vec::new(),
            tool_call_count,
        })
    }

    pub fn stats_for_session(
        _project_path: &str,
        session_id: &str,
    ) -> Option<crate::server::payload::SessionStats> {
        let db_path = find_db_path()?;
        extract_stats(&db_path, session_id)
    }

    pub fn read_transcript(path: &Path, max_messages: u32) -> String {
        let session_id = match extract_session_id_from_context(path) {
            Some(id) => id,
            None => return String::new(),
        };
        read_transcript_for_session(path, &session_id, max_messages)
    }

    pub fn read_transcript_for_session(
        db_path: &Path,
        session_id: &str,
        max_messages: u32,
    ) -> String {
        let messages = parse_messages_for_session(db_path, session_id);
        let start = if messages.len() > max_messages as usize {
            messages.len() - max_messages as usize
        } else {
            0
        };
        let mut out = String::new();
        for msg in &messages[start..] {
            let label = if msg.role == "user" {
                "User"
            } else {
                "Assistant"
            };
            out.push_str(&format!("── {label} ──\n{}\n\n", msg.text));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db(path: &Path) {
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

    #[test]
    fn test_sessions_from_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO session (id, title, message_count, prompt_tokens, completion_tokens, cost, created_at, updated_at) \
             VALUES ('sess-1', 'Fix login bug', 10, 5000, 3000, 0.15, 1700000000000, 1700000120000)",
            [],
        ).unwrap();

        let sessions = sessions_from_db(&db_path);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0.session_id, "sess-1");
        assert_eq!(sessions[0].0.name, "Fix login bug");
        assert_eq!(sessions[0].0.source, "opencode");
        assert_eq!(sessions[0].1.prompt_tokens, 5000);
        assert_eq!(sessions[0].1.completion_tokens, 3000);
    }

    #[test]
    fn test_count_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_test_db(&db_path);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO session (id, title, created_at, updated_at) VALUES ('s1', 'test', 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, role, content, created_at) VALUES ('m1', 's1', 'user', 'Hello', 1000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, role, content, created_at) VALUES ('m2', 's1', 'assistant', 'Hi there!', 2000)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, role, content, created_at) VALUES ('m3', 's1', 'tool', 'result', 3000)",
            [],
        ).unwrap();

        let msgs = transcript::parse_messages_for_session(&db_path, "s1");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "Hi there!");
    }

    #[test]
    fn test_extract_stats() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_test_db(&db_path);

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

        let stats = transcript::extract_stats(&db_path, "s1").unwrap();
        assert_eq!(stats.input_tokens, Some(15000));
        assert_eq!(stats.output_tokens, Some(8000));
        assert_eq!(stats.total_cost_usd, Some(0.45));
        assert_eq!(stats.tool_call_count, 2);
        assert_eq!(stats.duration_secs, Some(120));
    }

    #[test]
    fn test_empty_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_test_db(&db_path);

        let sessions = sessions_from_db(&db_path);
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_title_truncation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("opencode.db");
        create_test_db(&db_path);

        let long_title = "a".repeat(100);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO session (id, title, created_at, updated_at) VALUES ('s1', ?1, 0, 0)",
            [&long_title],
        )
        .unwrap();

        let sessions = sessions_from_db(&db_path);
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].0.name.len() <= 60);
        assert!(sessions[0].0.name.ends_with("..."));
    }
}
