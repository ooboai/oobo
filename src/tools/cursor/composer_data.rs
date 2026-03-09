#![allow(dead_code)]

use std::path::PathBuf;

use crate::analytics::NativeStats;
use crate::tools::cursor::transcript::Message;

/// Read full conversation data for a Cursor composer session from the global
/// state.vscdb `cursorDiskKV` table. This gives us access to the actual message
/// text (for tiktoken estimation), timestamps, files touched, and code blocks.
pub fn read_composer_data(session_id: &str) -> Option<ComposerSession> {
    let db_path = global_state_vscdb_path()?;
    if !db_path.exists() {
        return None;
    }

    let conn = crate::utils::open_db_readonly(&db_path).ok()?;

    let key = format!("composerData:{session_id}");
    let raw: String = conn
        .query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?1",
            [&key],
            |row| row.get(0),
        )
        .ok()?;

    let data: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parse_composer_session(&data)
}

/// Parsed Cursor composer session with extracted metadata.
#[derive(Debug, Clone)]
pub struct ComposerSession {
    pub composer_id: String,
    pub name: String,
    pub mode: String,
    pub status: String,
    pub created_at: Option<i64>,
    pub last_updated_at: Option<i64>,
    pub messages: Vec<Message>,
    pub files_touched: Vec<String>,
    pub code_block_count: u32,
    pub is_agentic: bool,
}

/// Extract native stats from a Cursor composerData entry.
/// Uses the session timestamps for duration and counts code blocks as tool calls.
///
/// Note: Cursor does not store which model was used per-session, so model is
/// always None here. Cost estimation for Cursor sessions is not possible
/// without the Cursor usage API (which requires browser-session auth).
pub fn extract_native_stats(session_id: &str) -> Option<NativeStats> {
    let session = read_composer_data(session_id)?;

    let duration_secs = match (session.created_at, session.last_updated_at) {
        (Some(c), Some(u)) if u > c => Some(((u - c) / 1000) as u64),
        _ => None,
    };

    Some(NativeStats {
        model: None,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        total_cost_usd: None,
        duration_secs,
        files_touched: session.files_touched,
        tool_call_count: session.code_block_count,
    })
}

/// Parse messages from a Cursor composerData entry for tiktoken processing.
/// Returns the same Message format used by the transcript parser.
pub fn parse_messages_from_composer(session_id: &str) -> Vec<Message> {
    read_composer_data(session_id)
        .map(|s| s.messages)
        .unwrap_or_default()
}

/// Load composerData entries for specific sessions from Cursor's state.vscdb.
/// Only fetches and parses data for the given session IDs.
pub fn preload_composer_data_for(
    session_ids: &[String],
) -> std::collections::HashMap<String, ComposerSession> {
    let mut map = std::collections::HashMap::new();

    if session_ids.is_empty() {
        return map;
    }

    let db_path = match global_state_vscdb_path() {
        Some(p) if p.exists() => p,
        _ => return map,
    };

    let conn = match crate::utils::open_db_readonly(&db_path) {
        Ok(c) => c,
        Err(_) => return map,
    };

    let mut stmt = match conn.prepare("SELECT value FROM cursorDiskKV WHERE key = ?1") {
        Ok(s) => s,
        Err(_) => return map,
    };

    for session_id in session_ids {
        let key = format!("composerData:{session_id}");
        let raw: String = match stmt.query_row(rusqlite::params![&key], |row| row.get(0)) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(session) = parse_composer_session(&data) {
                map.insert(session_id.clone(), session);
            }
        }
    }

    map
}

/// Load conversation data from the new `bubbleId:` format for specific sessions.
/// Uses SQLite json_extract to avoid deserializing full bubble JSON (which
/// includes massive arrays of diffs, lints, code blocks, etc.).
pub fn preload_bubble_data_for(
    session_ids: &[String],
) -> std::collections::HashMap<String, BubbleSession> {
    let mut map: std::collections::HashMap<String, BubbleSession> =
        std::collections::HashMap::new();

    if session_ids.is_empty() {
        return map;
    }

    let db_path = match global_state_vscdb_path() {
        Some(p) if p.exists() => p,
        _ => return map,
    };

    let conn = match crate::utils::open_db_readonly(&db_path) {
        Ok(c) => c,
        Err(_) => return map,
    };

    let mut stmt = match conn.prepare(
        "SELECT key,
                json_extract(value, '$.type') AS btype,
                json_extract(value, '$.text') AS btext,
                json_extract(value, '$.tokenCount.inputTokens') AS inp,
                json_extract(value, '$.tokenCount.outputTokens') AS outp,
                json_extract(value, '$.createdAt') AS created
         FROM cursorDiskKV
         WHERE key >= ?1 AND key < ?2",
    ) {
        Ok(s) => s,
        Err(_) => return map,
    };

    for session_id in session_ids {
        let prefix = format!("bubbleId:{session_id}:");
        let prefix_end = format!("bubbleId:{session_id};");

        let rows = match stmt.query_map(rusqlite::params![&prefix, &prefix_end], |row| {
            Ok((
                row.get::<_, i64>(1).unwrap_or(0),
                row.get::<_, Option<String>>(2).unwrap_or(None),
                row.get::<_, i64>(3).unwrap_or(0),
                row.get::<_, i64>(4).unwrap_or(0),
                row.get::<_, Option<String>>(5).unwrap_or(None),
            ))
        }) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let mut entry = BubbleSession::default();

        for row in rows {
            let (btype, text_opt, inp, outp, created) = match row {
                Ok(r) => r,
                Err(_) => continue,
            };

            let role = match btype {
                1 => "user",
                2 => "assistant",
                _ => continue,
            };

            let text = text_opt.unwrap_or_default();
            let timestamp_ms = created
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());

            if !text.is_empty() {
                entry.messages.push(Message {
                    role: role.to_string(),
                    text,
                    timestamp_ms,
                });
            }

            entry.total_input_tokens += inp as u64;
            entry.total_output_tokens += outp as u64;
        }

        if !entry.messages.is_empty() {
            map.insert(session_id.clone(), entry);
        }
    }

    map
}

#[derive(Debug, Clone, Default)]
pub struct BubbleSession {
    pub messages: Vec<Message>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub files_touched: Vec<String>,
    pub code_block_count: u32,
    pub tool_call_count: u32,
    pub is_agentic: bool,
}

pub fn native_stats_from_bubble(session: &BubbleSession) -> NativeStats {
    let has_native = session.total_input_tokens > 0 || session.total_output_tokens > 0;
    NativeStats {
        model: None,
        input_tokens: if has_native {
            Some(session.total_input_tokens)
        } else {
            None
        },
        output_tokens: if has_native {
            Some(session.total_output_tokens)
        } else {
            None
        },
        cache_read_tokens: None,
        cache_creation_tokens: None,
        total_cost_usd: None,
        duration_secs: None,
        files_touched: session.files_touched.clone(),
        tool_call_count: session.tool_call_count + session.code_block_count,
    }
}

/// Extract native stats from a preloaded ComposerSession (avoids re-reading the DB).
pub fn native_stats_from_session(session: &ComposerSession) -> NativeStats {
    let duration_secs = match (session.created_at, session.last_updated_at) {
        (Some(c), Some(u)) if u > c => Some(((u - c) / 1000) as u64),
        _ => None,
    };

    NativeStats {
        model: None,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        total_cost_usd: None,
        duration_secs,
        files_touched: session.files_touched.clone(),
        tool_call_count: session.code_block_count,
    }
}

fn parse_composer_session(data: &serde_json::Value) -> Option<ComposerSession> {
    let composer_id = data.get("composerId").and_then(|v| v.as_str())?.to_string();

    let name = data
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mode = data
        .get("forceMode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let status = data
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let created_at = data.get("createdAt").and_then(|v| v.as_i64());
    let last_updated_at = data.get("lastUpdatedAt").and_then(|v| v.as_i64());

    let conversation = data.get("conversation").and_then(|v| v.as_array());

    let mut messages = Vec::new();
    let mut files_touched = Vec::new();
    let mut code_block_count = 0u32;
    let mut is_agentic = false;

    if let Some(conv) = conversation {
        for msg in conv {
            let msg_type = msg.get("type").and_then(|v| v.as_u64()).unwrap_or(0);
            let text = msg
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if msg
                .get("isAgentic")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                is_agentic = true;
            }

            let role = match msg_type {
                1 => "user",
                2 => "assistant",
                _ => continue,
            };

            if !text.is_empty() {
                messages.push(Message {
                    role: role.to_string(),
                    text,
                    timestamp_ms: None,
                });
            }

            if let Some(code_blocks) = msg.get("codeBlocks").and_then(|v| v.as_array()) {
                code_block_count += code_blocks.len() as u32;
            }
            if let Some(suggested) = msg.get("suggestedCodeBlocks").and_then(|v| v.as_array()) {
                code_block_count += suggested.len() as u32;
            }

            if let Some(relevant) = msg.get("relevantFiles").and_then(|v| v.as_array()) {
                for f in relevant {
                    if let Some(name) = f.as_str() {
                        let s = name.to_string();
                        if !files_touched.contains(&s) {
                            files_touched.push(s);
                        }
                    } else if let Some(name) = f.get("path").and_then(|v| v.as_str()) {
                        let s = name.to_string();
                        if !files_touched.contains(&s) {
                            files_touched.push(s);
                        }
                    }
                }
            }
        }
    }

    if let Some(new_files) = data.get("newlyCreatedFiles").and_then(|v| v.as_array()) {
        for f in new_files {
            if let Some(name) = f.as_str() {
                let s = name.to_string();
                if !files_touched.contains(&s) {
                    files_touched.push(s);
                }
            }
        }
    }

    Some(ComposerSession {
        composer_id,
        name,
        mode,
        status,
        created_at,
        last_updated_at,
        messages,
        files_touched,
        code_block_count,
        is_agentic,
    })
}

/// Read all Cursor daily AI code tracking stats from the global state.vscdb.
/// Returns entries sorted by date descending.
pub fn read_daily_code_stats() -> Vec<DailyCodeStats> {
    let db_path = match global_state_vscdb_path() {
        Some(p) if p.exists() => p,
        _ => return Vec::new(),
    };

    let conn = match crate::utils::open_db_readonly(&db_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut stmt = match conn.prepare(
        "SELECT key, value FROM ItemTable WHERE key LIKE 'aiCodeTracking.dailyStats.%' ORDER BY key DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let rows = match stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();
    for (_key, value) in rows.flatten() {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&value) {
            let date = data
                .get("date")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if date.is_empty() {
                continue;
            }
            results.push(DailyCodeStats {
                date,
                tab_suggested_lines: data
                    .get("tabSuggestedLines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                tab_accepted_lines: data
                    .get("tabAcceptedLines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                composer_suggested_lines: data
                    .get("composerSuggestedLines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                composer_accepted_lines: data
                    .get("composerAcceptedLines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            });
        }
    }

    results
}

/// Cursor's daily AI code tracking stats.
#[derive(Debug, Clone)]
pub struct DailyCodeStats {
    pub date: String,
    pub tab_suggested_lines: u64,
    pub tab_accepted_lines: u64,
    pub composer_suggested_lines: u64,
    pub composer_accepted_lines: u64,
}

impl DailyCodeStats {
    pub fn total_suggested(&self) -> u64 {
        self.tab_suggested_lines + self.composer_suggested_lines
    }

    pub fn total_accepted(&self) -> u64 {
        self.tab_accepted_lines + self.composer_accepted_lines
    }

    pub fn acceptance_rate(&self) -> f64 {
        let total = self.total_suggested();
        if total == 0 {
            return 0.0;
        }
        self.total_accepted() as f64 / total as f64
    }
}

fn global_state_vscdb_path() -> Option<PathBuf> {
    super::state_vscdb_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_composer_session_basic() {
        let data = serde_json::json!({
            "composerId": "abc-123",
            "name": "Fix auth",
            "forceMode": "agent",
            "status": "completed",
            "createdAt": 1700000000000_i64,
            "lastUpdatedAt": 1700000060000_i64,
            "conversation": [
                {
                    "type": 1,
                    "text": "Fix the auth module",
                    "isAgentic": false,
                    "bubbleId": "b1",
                    "relevantFiles": [],
                    "suggestedCodeBlocks": []
                },
                {
                    "type": 2,
                    "text": "I'll fix the auth module by updating the token validation...",
                    "isAgentic": true,
                    "bubbleId": "b2",
                    "codeBlocks": [{"uri": "file:///src/auth.rs"}],
                    "suggestedCodeBlocks": [{"uri": "file:///src/auth.rs"}],
                    "relevantFiles": ["src/auth.rs", "src/main.rs"]
                }
            ],
            "newlyCreatedFiles": ["src/middleware.rs"]
        });

        let session = parse_composer_session(&data).unwrap();
        assert_eq!(session.composer_id, "abc-123");
        assert_eq!(session.name, "Fix auth");
        assert_eq!(session.mode, "agent");
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, "user");
        assert_eq!(session.messages[1].role, "assistant");
        assert!(session.is_agentic);
        assert_eq!(session.code_block_count, 2);
        assert!(session.files_touched.contains(&"src/auth.rs".to_string()));
        assert!(session.files_touched.contains(&"src/main.rs".to_string()));
        assert!(session
            .files_touched
            .contains(&"src/middleware.rs".to_string()));
        assert_eq!(session.created_at, Some(1700000000000));
        assert_eq!(session.last_updated_at, Some(1700000060000));
    }

    #[test]
    fn test_parse_composer_session_empty_conversation() {
        let data = serde_json::json!({
            "composerId": "def-456",
            "name": "",
            "forceMode": "chat",
            "status": "none",
            "createdAt": 1700000000000_i64,
            "conversation": []
        });

        let session = parse_composer_session(&data).unwrap();
        assert!(session.messages.is_empty());
        assert!(!session.is_agentic);
    }

    #[test]
    fn test_extract_native_stats_duration() {
        let data = serde_json::json!({
            "composerId": "test-123",
            "createdAt": 1700000000000_i64,
            "lastUpdatedAt": 1700000120000_i64,
            "conversation": [
                {"type": 1, "text": "hello"},
                {"type": 2, "text": "hi", "codeBlocks": [{}]}
            ]
        });

        let session = parse_composer_session(&data).unwrap();
        let duration = match (session.created_at, session.last_updated_at) {
            (Some(c), Some(u)) if u > c => Some(((u - c) / 1000) as u64),
            _ => None,
        };
        assert_eq!(duration, Some(120));
    }

    #[test]
    fn test_daily_code_stats_rate() {
        let stats = DailyCodeStats {
            date: "2026-03-05".to_string(),
            tab_suggested_lines: 100,
            tab_accepted_lines: 40,
            composer_suggested_lines: 200,
            composer_accepted_lines: 60,
        };
        assert_eq!(stats.total_suggested(), 300);
        assert_eq!(stats.total_accepted(), 100);
        let rate = stats.acceptance_rate();
        assert!((rate - 0.333).abs() < 0.01);
    }
}
