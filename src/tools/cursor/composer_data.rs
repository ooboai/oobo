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
                json_extract(value, '$.createdAt') AS created,
                json_extract(value, '$.thinking.text') AS thinking,
                json_extract(value, '$.toolFormerData.name') AS tool_name,
                json_extract(value, '$.toolFormerData.result') AS tool_result
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
                row.get::<_, Option<String>>(6).unwrap_or(None),
                row.get::<_, Option<String>>(7).unwrap_or(None),
                row.get::<_, Option<String>>(8).unwrap_or(None),
            ))
        }) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let mut entry = BubbleSession::default();
        let mut bubble_count = 0u32;

        for row in rows {
            let (btype, text_opt, inp, outp, created, thinking_opt, tool_name_opt, tool_result_opt) =
                match row {
                    Ok(r) => r,
                    Err(_) => continue,
                };

            let role = match btype {
                1 => "user",
                2 => "assistant",
                _ => continue,
            };

            bubble_count += 1;

            let text = text_opt.unwrap_or_default();
            let thinking_text = thinking_opt.unwrap_or_default();
            let tool_result = tool_result_opt.unwrap_or_default();
            let timestamp_ms = created
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());

            // Combine visible text + thinking block for assistant output estimation.
            let combined = if !text.is_empty() && !thinking_text.is_empty() {
                format!("{text}\n{thinking_text}")
            } else if !thinking_text.is_empty() {
                thinking_text
            } else {
                text
            };

            if !combined.is_empty() {
                entry.messages.push(Message {
                    role: role.to_string(),
                    text: combined,
                    timestamp_ms,
                });
            }

            // Tool results (file contents, terminal output, etc.) are fed back to
            // the model as context — count them as input tokens via "user" role.
            if !tool_result.is_empty() {
                entry.messages.push(Message {
                    role: "user".to_string(),
                    text: tool_result,
                    timestamp_ms,
                });
            }

            if tool_name_opt.is_some() {
                entry.tool_call_count += 1;
            }

            entry.total_input_tokens += inp as u64;
            entry.total_output_tokens += outp as u64;
        }

        if !entry.messages.is_empty() || bubble_count > 0 {
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

/// Extract file paths edited by a Cursor session from its bubbleId: DB entries.
/// Returns repo-relative paths (e.g. `src/main.rs`) by stripping `project_root`.
///
/// `since_epoch` filters to only edits after that Unix timestamp (seconds).
/// Pass 0 to include all edits.
pub fn files_edited_in_session(
    session_id: &str,
    project_root: &str,
    since_epoch: i64,
) -> Vec<String> {
    let db_path = match global_state_vscdb_path() {
        Some(p) if p.exists() => p,
        _ => return Vec::new(),
    };

    let conn = match crate::utils::open_db_readonly(&db_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let bubble_ids = match read_bubble_order(&conn, session_id) {
        Some(ids) => ids,
        None => return Vec::new(),
    };

    let mut stmt = match conn.prepare(
        "SELECT json_extract(value, '$.toolFormerData.name'),
                json_extract(value, '$.toolFormerData.params'),
                json_extract(value, '$.createdAt')
         FROM cursorDiskKV WHERE key = ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for bubble_id in &bubble_ids {
        let key = format!("bubbleId:{session_id}:{bubble_id}");
        let row: Option<(Option<String>, Option<String>, Option<String>)> = stmt
            .query_row(rusqlite::params![&key], |row| {
                Ok((row.get(0).ok(), row.get(1).ok(), row.get(2).ok()))
            })
            .ok();

        if let Some((Some(tool_name), Some(params_str), created_at)) = row {
            if tool_name == "edit_file_v2" {
                if since_epoch > 0 {
                    if let Some(ts_str) = &created_at {
                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                            if dt.timestamp() <= since_epoch {
                                continue;
                            }
                        }
                    }
                }

                if let Ok(params) = serde_json::from_str::<serde_json::Value>(&params_str) {
                    if let Some(raw_path) =
                        params.get("relativeWorkspacePath").and_then(|v| v.as_str())
                    {
                        let rel = normalize_to_repo_relative(raw_path, project_root);
                        if seen.insert(rel.clone()) {
                            files.push(rel);
                        }
                    }
                }
            }
        }
    }

    files
}

/// Normalize a tool-reported path to a repo-relative path.
/// Handles: absolute paths under project root, workspace-relative paths
/// with leading `/`, and already-relative paths.
fn normalize_to_repo_relative(file_path: &str, project_root: &str) -> String {
    let p = std::path::Path::new(file_path);
    if let Ok(rel) = p.strip_prefix(project_root) {
        return rel.to_string_lossy().to_string();
    }
    // Strip leading `/` — Cursor's relativeWorkspacePath sometimes includes
    // it. If it's truly from another project, it won't match committed files.
    file_path.strip_prefix('/').unwrap_or(file_path).to_string()
}

/// Build a rich JSONL transcript from Cursor's bubbleId: database entries.
/// Includes thinking blocks, tool calls (with params), timestamps, and tokens
/// — the full agent reasoning and actions timeline, not just chat text.
pub fn build_rich_transcript(session_id: &str) -> Option<String> {
    let db_path = global_state_vscdb_path()?;
    if !db_path.exists() {
        return None;
    }

    let conn = crate::utils::open_db_readonly(&db_path).ok()?;

    let bubble_order = read_bubble_order(&conn, session_id)?;
    if bubble_order.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    let mut stmt = conn
        .prepare("SELECT value FROM cursorDiskKV WHERE key = ?1")
        .ok()?;

    for bubble_id in &bubble_order {
        let key = format!("bubbleId:{session_id}:{bubble_id}");
        let raw: String = match stmt.query_row(rusqlite::params![&key], |row| row.get(0)) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let data: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(entry) = bubble_to_transcript_entry(&data) {
            if let Ok(json) = serde_json::to_string(&entry) {
                lines.push(json);
            }
        }
    }

    if lines.is_empty() {
        return None;
    }

    Some(lines.join("\n"))
}

fn read_bubble_order(conn: &rusqlite::Connection, session_id: &str) -> Option<Vec<String>> {
    let key = format!("composerData:{session_id}");
    let raw: String = conn
        .query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?1",
            [&key],
            |row| row.get(0),
        )
        .ok()?;

    let data: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let headers = data
        .get("fullConversationHeadersOnly")
        .and_then(|v| v.as_array())?;

    let ids: Vec<String> = headers
        .iter()
        .filter_map(|h| h.get("bubbleId").and_then(|v| v.as_str()).map(String::from))
        .collect();

    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

fn bubble_to_transcript_entry(data: &serde_json::Value) -> Option<serde_json::Value> {
    let btype = data.get("type").and_then(|v| v.as_u64())?;
    let role = match btype {
        1 => "user",
        2 => "assistant",
        _ => return None,
    };

    let mut entry = serde_json::Map::new();
    entry.insert("role".into(), role.into());

    if let Some(ts) = data.get("createdAt").and_then(|v| v.as_str()) {
        entry.insert("timestamp".into(), ts.into());
    }

    let text = data
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !text.is_empty() {
        entry.insert("text".into(), text.into());
    }

    if let Some(thinking) = data.get("thinking") {
        if let Some(think_text) = thinking.get("text").and_then(|v| v.as_str()) {
            if !think_text.is_empty() {
                let mut think_obj = serde_json::Map::new();
                think_obj.insert("text".into(), think_text.into());
                if let Some(ms) = data.get("thinkingDurationMs").and_then(|v| v.as_u64()) {
                    think_obj.insert("duration_ms".into(), ms.into());
                }
                entry.insert("thinking".into(), think_obj.into());
            }
        }
    }

    if let Some(tfd) = data.get("toolFormerData").and_then(|v| v.as_object()) {
        let tool_name = tfd.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if !tool_name.is_empty() {
            let mut tool_obj = serde_json::Map::new();
            tool_obj.insert("name".into(), tool_name.into());

            if let Some(status) = tfd.get("status").and_then(|v| v.as_str()) {
                tool_obj.insert("status".into(), status.into());
            }

            if let Some(params_str) = tfd.get("params").and_then(|v| v.as_str()) {
                if let Ok(params) = serde_json::from_str::<serde_json::Value>(params_str) {
                    tool_obj.insert("params".into(), summarize_tool_params(tool_name, &params));
                }
            }

            entry.insert("tool_call".into(), tool_obj.into());
        }
    }

    if let Some(tc) = data.get("tokenCount").and_then(|v| v.as_object()) {
        let inp = tc.get("inputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let outp = tc.get("outputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
        if inp > 0 || outp > 0 {
            entry.insert(
                "tokens".into(),
                serde_json::json!({"input": inp, "output": outp}),
            );
        }
    }

    if entry.len() <= 2 {
        return None;
    }

    Some(entry.into())
}

/// Summarize tool call params — keep file paths and commands, trim large content.
fn summarize_tool_params(tool_name: &str, params: &serde_json::Value) -> serde_json::Value {
    let obj = match params.as_object() {
        Some(o) => o,
        None => return params.clone(),
    };

    let mut summary = serde_json::Map::new();

    match tool_name {
        "edit_file_v2" => {
            if let Some(path) = obj.get("relativeWorkspacePath") {
                summary.insert("path".into(), path.clone());
            }
        }
        "read_file_v2" => {
            if let Some(path) = obj.get("relativeWorkspacePath") {
                summary.insert("path".into(), path.clone());
            }
        }
        "run_terminal_command_v2" => {
            if let Some(cmd) = obj.get("command") {
                summary.insert("command".into(), cmd.clone());
            }
            if let Some(cwd) = obj.get("cwd") {
                summary.insert("cwd".into(), cwd.clone());
            }
        }
        _ => {
            for (k, v) in obj {
                if k == "streamingContent" || k == "toolCallBinary" {
                    continue;
                }
                let val_str = v.to_string();
                if val_str.len() > 500 {
                    summary.insert(k.clone(), format!("({} chars)", val_str.len()).into());
                } else {
                    summary.insert(k.clone(), v.clone());
                }
            }
        }
    }

    summary.into()
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

    #[test]
    fn test_bubble_to_transcript_user_message() {
        let data = serde_json::json!({
            "type": 1,
            "text": "fix the auth module",
            "createdAt": "2026-03-11T02:51:53.895Z",
            "tokenCount": {"inputTokens": 0, "outputTokens": 0},
        });
        let entry = bubble_to_transcript_entry(&data).unwrap();
        assert_eq!(entry["role"], "user");
        assert_eq!(entry["text"], "fix the auth module");
        assert_eq!(entry["timestamp"], "2026-03-11T02:51:53.895Z");
        assert!(entry.get("thinking").is_none());
        assert!(entry.get("tool_call").is_none());
    }

    #[test]
    fn test_bubble_to_transcript_thinking() {
        let data = serde_json::json!({
            "type": 2,
            "text": "",
            "createdAt": "2026-03-11T02:51:56.314Z",
            "thinking": {"text": "Let me analyze this code", "signature": "sig"},
            "thinkingDurationMs": 1500,
            "tokenCount": {"inputTokens": 100, "outputTokens": 50},
        });
        let entry = bubble_to_transcript_entry(&data).unwrap();
        assert_eq!(entry["role"], "assistant");
        assert_eq!(entry["thinking"]["text"], "Let me analyze this code");
        assert_eq!(entry["thinking"]["duration_ms"], 1500);
        assert_eq!(entry["tokens"]["input"], 100);
        assert_eq!(entry["tokens"]["output"], 50);
    }

    #[test]
    fn test_bubble_to_transcript_tool_call() {
        let data = serde_json::json!({
            "type": 2,
            "createdAt": "2026-03-11T02:51:57.000Z",
            "toolFormerData": {
                "name": "edit_file_v2",
                "status": "completed",
                "params": "{\"relativeWorkspacePath\":\"/src/main.rs\",\"streamingContent\":\"huge file content here...\"}",
            },
            "tokenCount": {"inputTokens": 0, "outputTokens": 0},
        });
        let entry = bubble_to_transcript_entry(&data).unwrap();
        assert_eq!(entry["role"], "assistant");
        assert_eq!(entry["tool_call"]["name"], "edit_file_v2");
        assert_eq!(entry["tool_call"]["status"], "completed");
        assert_eq!(entry["tool_call"]["params"]["path"], "/src/main.rs");
        assert!(entry["tool_call"]["params"]
            .get("streamingContent")
            .is_none());
    }

    #[test]
    fn test_bubble_to_transcript_terminal_command() {
        let data = serde_json::json!({
            "type": 2,
            "createdAt": "2026-03-11T02:52:27.000Z",
            "toolFormerData": {
                "name": "run_terminal_command_v2",
                "status": "completed",
                "params": "{\"command\":\"oobo commit -m 'test'\",\"cwd\":\"/Users/teddy/projects\"}",
            },
            "tokenCount": {"inputTokens": 0, "outputTokens": 0},
        });
        let entry = bubble_to_transcript_entry(&data).unwrap();
        assert_eq!(entry["tool_call"]["name"], "run_terminal_command_v2");
        assert_eq!(
            entry["tool_call"]["params"]["command"],
            "oobo commit -m 'test'"
        );
    }

    #[test]
    fn test_bubble_to_transcript_empty_bubble_skipped() {
        let data = serde_json::json!({
            "type": 2,
            "text": "",
            "createdAt": "2026-03-11T02:52:00.000Z",
            "tokenCount": {"inputTokens": 0, "outputTokens": 0},
        });
        assert!(bubble_to_transcript_entry(&data).is_none());
    }

    #[test]
    fn test_summarize_tool_params_strips_content() {
        let params = serde_json::json!({
            "relativeWorkspacePath": "/src/main.rs",
            "streamingContent": "fn main() { /* 10000 lines */ }",
            "noCodeblock": true,
        });
        let summary = summarize_tool_params("edit_file_v2", &params);
        assert_eq!(summary["path"], "/src/main.rs");
        assert!(summary.get("streamingContent").is_none());
        assert!(summary.get("noCodeblock").is_none());
    }

    #[test]
    fn test_normalize_to_repo_relative_absolute() {
        let root = "/Users/teddy/dev/projects/trender";
        let abs = "/Users/teddy/dev/projects/trender/backdate.sh";
        assert_eq!(normalize_to_repo_relative(abs, root), "backdate.sh");
    }

    #[test]
    fn test_normalize_to_repo_relative_nested() {
        let root = "/Users/teddy/dev/projects/trender";
        let abs = "/Users/teddy/dev/projects/trender/src/main.rs";
        assert_eq!(normalize_to_repo_relative(abs, root), "src/main.rs");
    }

    #[test]
    fn test_normalize_to_repo_relative_leading_slash() {
        let root = "/Users/teddy/dev/projects/trender";
        let rel = "/src/main.rs";
        assert_eq!(normalize_to_repo_relative(rel, root), "src/main.rs");
    }

    #[test]
    fn test_normalize_to_repo_relative_already_relative() {
        let root = "/Users/teddy/dev/projects/trender";
        let rel = "src/main.rs";
        assert_eq!(normalize_to_repo_relative(rel, root), "src/main.rs");
    }

    #[test]
    fn test_normalize_to_repo_relative_different_root() {
        let root = "/Users/teddy/dev/projects/trender";
        let abs = "/Users/teddy/dev/projects/other/file.rs";
        // Stripped leading `/` — won't match committed files anyway.
        assert_eq!(
            normalize_to_repo_relative(abs, root),
            "Users/teddy/dev/projects/other/file.rs"
        );
    }
}
