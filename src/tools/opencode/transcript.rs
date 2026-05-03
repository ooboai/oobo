use std::path::{Path, PathBuf};

use crate::tools::cursor::transcript::Message;

use super::{DbSchema, detect_schema, find_db_path, stats_from_legacy_db, stats_from_modern_db};

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
    let msg_ids: Vec<String> = rows.filter_map(std::result::Result::ok).collect();

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
            Ok(rows) => rows.filter_map(std::result::Result::ok).collect(),
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

#[cfg(test)]
pub fn read_transcript(path: &Path, max_messages: u32) -> String {
    let session_id = match extract_session_id_from_context(path) {
        Some(id) => id,
        None => return String::new(),
    };
    read_transcript_for_session(path, &session_id, max_messages)
}

#[cfg(test)]
pub fn read_transcript_for_session(
    db_path: &Path,
    session_id: &str,
    max_messages: u32,
) -> String {
    let messages = parse_messages_for_session(db_path, session_id);
    crate::utils::format_transcript(&messages, max_messages, "Assistant")
}
