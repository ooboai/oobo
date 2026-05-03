use std::path::Path;

use crate::tools::cursor::Session;

use super::normalize_ts;

/// Legacy token/cost data from OpenCode's old schema. Populated during
/// session discovery; callers currently discard it (kept for potential
/// analytics enrichment).
#[allow(dead_code)]
pub(crate) struct SessionMeta {
    pub(super) message_count: u32,
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) cost: f64,
}

pub(crate) fn sessions_from_legacy_db(
    conn: &rusqlite::Connection,
    db_path: &Path,
) -> Vec<(Session, SessionMeta)> {
    let has_parent_col = conn
        .prepare("SELECT parent_session_id FROM session LIMIT 0")
        .is_ok();

    let query = if has_parent_col {
        "SELECT id, title, message_count, prompt_tokens, completion_tokens, \
         cost, created_at, updated_at, parent_session_id \
         FROM session ORDER BY updated_at DESC"
    } else {
        "SELECT id, title, message_count, prompt_tokens, completion_tokens, \
         cost, created_at, updated_at \
         FROM session ORDER BY updated_at DESC"
    };

    let mut stmt = match conn.prepare(query) {
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
        let parent_session_id: Option<String> = if has_parent_col {
            row.get(8).ok().flatten()
        } else {
            None
        };
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
            parent_session_id,
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
                parent_session_id,
                subagent_type: None,
            },
            meta,
        ));
    }

    sessions
}

pub(crate) fn stats_from_legacy_db(
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
