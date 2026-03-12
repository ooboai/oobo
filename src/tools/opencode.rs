use std::path::{Path, PathBuf};

use crate::tools::cursor::transcript::Message;
use crate::tools::cursor::Session;

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

enum DbSchema {
    /// Old schema: session table has prompt_tokens, completion_tokens, cost columns.
    Legacy,
    /// New schema (v1.2+): tokens live in message.data JSON, projects in separate table.
    Modern,
}

fn detect_schema(conn: &rusqlite::Connection) -> DbSchema {
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
    let mut stmt = match conn.prepare(
        "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated, p.worktree \
         FROM session s \
         LEFT JOIN project p ON s.project_id = p.id \
         WHERE s.time_archived IS NULL \
         ORDER BY s.time_updated DESC",
    ) {
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
        Ok((id, title, directory, time_created, time_updated, worktree))
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for row in rows.flatten() {
        let (id, title, directory, time_created, time_updated, worktree) = row;

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
        });
    }

    sessions
}

/// Aggregate per-message native token data from message.data JSON.
fn stats_from_modern_db(
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
            input_tokens += tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
            output_tokens += tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
            if let Some(cache) = tokens.get("cache") {
                cache_read += cache.get("read").and_then(|v| v.as_u64()).unwrap_or(0);
                cache_write += cache.get("write").and_then(|v| v.as_u64()).unwrap_or(0);
            }
        }

        if model.is_none() {
            model = v
                .get("modelID")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string());
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

// Legacy schema (pre v1.2)

/// Legacy token/cost data carried for test assertions; production callers discard it.
#[allow(dead_code)]
struct SessionMeta {
    message_count: u32,
    prompt_tokens: u64,
    completion_tokens: u64,
    cost: f64,
}

fn sessions_from_legacy_db(
    conn: &rusqlite::Connection,
    db_path: &Path,
) -> Vec<(Session, SessionMeta)> {
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
        Ok((
            id,
            title,
            message_count,
            prompt_tokens,
            completion_tokens,
            cost,
            created_at,
            updated_at,
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
        ) = row;

        let name = if title.is_empty() {
            "OpenCode session".to_string()
        } else {
            crate::utils::truncate_name(&title, crate::utils::MAX_SESSION_NAME_LEN)
        };

        let created_ms = normalize_ts(created_at);
        let updated_ms = normalize_ts(updated_at);

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

fn stats_from_legacy_db(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Option<crate::remote::payload::SessionStats> {
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

    let (prompt_tokens, completion_tokens, _cost, created_at, updated_at) = row;

    let created_ms = normalize_ts(created_at);
    let updated_ms = normalize_ts(updated_at);
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
            "SELECT COUNT(*) FROM message WHERE session_id = ?1 AND role = 'tool'",
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

    Some(crate::remote::payload::SessionStats {
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
        duration_secs,
        files_touched: Vec::new(),
        tool_call_count,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        is_estimated: false,
        token_source: None,
    })
}

// Helpers

fn normalize_ts(ts: i64) -> i64 {
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

pub mod transcript {
    use super::*;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        let db_path = find_db_path()?;
        let conn = crate::utils::open_db_readonly(&db_path).ok()?;

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

        let conn = match crate::utils::open_db_readonly(&db_path) {
            Ok(c) => c,
            Err(_) => return 0,
        };

        match detect_schema(&conn) {
            DbSchema::Modern => conn
                .query_row(
                    "SELECT COUNT(*) FROM message WHERE session_id = ?1",
                    [session_id],
                    |row| row.get::<_, u32>(0),
                )
                .unwrap_or(0),
            DbSchema::Legacy => conn
                .query_row(
                    "SELECT COUNT(*) FROM message WHERE session_id = ?1 AND role IN ('user', 'assistant')",
                    [session_id],
                    |row| row.get::<_, u32>(0),
                )
                .unwrap_or(0),
        }
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let session_id = match extract_session_id_from_context(path) {
            Some(id) => id,
            None => return Vec::new(),
        };
        parse_messages_for_session(path, &session_id)
    }

    pub fn parse_messages_for_session(db_path: &Path, session_id: &str) -> Vec<Message> {
        let conn = match crate::utils::open_db_readonly(db_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        match detect_schema(&conn) {
            DbSchema::Modern => parse_modern_messages(&conn, session_id),
            DbSchema::Legacy => parse_legacy_messages(&conn, session_id),
        }
    }

    fn parse_modern_messages(conn: &rusqlite::Connection, session_id: &str) -> Vec<Message> {
        let mut stmt = match conn
            .prepare("SELECT data FROM message WHERE session_id = ?1 ORDER BY time_created ASC")
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

        let mut messages = Vec::new();
        let msg_ids: Vec<String> = rows.filter_map(|r| r.ok()).collect();

        for data_str in &msg_ids {
            let v: serde_json::Value = match serde_json::from_str(data_str) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role != "user" && role != "assistant" {
                continue;
            }

            messages.push(Message {
                role: role.to_string(),
                text: String::new(),
                timestamp_ms: None,
            });
        }

        let mut part_stmt = match conn.prepare(
            "SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY time_created ASC",
        ) {
            Ok(s) => s,
            Err(_) => return messages,
        };

        if let Ok(part_rows) = part_stmt.query_map([session_id], |row| {
            let mid: String = row.get(0)?;
            let data: String = row.get(1)?;
            Ok((mid, data))
        }) {
            let mut msg_stmt = match conn.prepare(
                "SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created ASC",
            ) {
                Ok(s) => s,
                Err(_) => return messages,
            };
            let msg_map: Vec<(String, String)> = match msg_stmt.query_map([session_id], |row| {
                let id: String = row.get(0)?;
                let data: String = row.get(1)?;
                Ok((id, data))
            }) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => return messages,
            };

            let mut msg_text: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();

            for row in part_rows.flatten() {
                let (mid, data_str) = row;
                let v: serde_json::Value = match serde_json::from_str(&data_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let t = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if t == "text" {
                    if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                        msg_text.entry(mid).or_default().push(text.to_string());
                    }
                }
            }

            let mut idx = 0;
            for (mid, data_str) in &msg_map {
                let v: serde_json::Value = match serde_json::from_str(data_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if role != "user" && role != "assistant" {
                    continue;
                }
                if idx < messages.len() {
                    if let Some(texts) = msg_text.get(mid) {
                        messages[idx].text = texts.join("\n");
                    }
                }
                idx += 1;
            }
        }

        messages.retain(|m| !m.text.is_empty());
        messages
    }

    fn parse_legacy_messages(conn: &rusqlite::Connection, session_id: &str) -> Vec<Message> {
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
                messages.push(Message {
                    role,
                    text,
                    timestamp_ms: None,
                });
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
    ) -> Option<crate::remote::payload::SessionStats> {
        let conn = crate::utils::open_db_readonly(db_path).ok()?;

        match detect_schema(&conn) {
            DbSchema::Modern => stats_from_modern_db(&conn, session_id),
            DbSchema::Legacy => stats_from_legacy_db(&conn, session_id),
        }
    }

    pub fn stats_for_session(
        _project_path: &str,
        session_id: &str,
    ) -> Option<crate::remote::payload::SessionStats> {
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
        crate::utils::format_transcript(&messages, max_messages, "Assistant")
    }
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
