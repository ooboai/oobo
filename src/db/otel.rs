#![allow(dead_code)]

use rusqlite::params;

use super::Db;

#[derive(Debug, Clone)]
pub struct OtelEventRow {
    pub event_name: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    pub tool_name: Option<String>,
    pub tool_success: Option<bool>,
    pub prompt_length: Option<i64>,
    pub account_uuid: Option<String>,
    pub timestamp: i64,
    pub raw_attributes: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct OtelSummary {
    pub event_count: i64,
    pub total_cost_usd: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_duration_ms: i64,
    pub tool_calls: i64,
    pub api_requests: i64,
}

impl Db {
    pub fn insert_otel_event(&self, row: &OtelEventRow) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO otel_events (event_name, session_id, model, cost_usd,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    duration_ms, tool_name, tool_success, prompt_length,
                    account_uuid, timestamp, raw_attributes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    row.event_name,
                    row.session_id,
                    row.model,
                    row.cost_usd,
                    row.input_tokens,
                    row.output_tokens,
                    row.cache_read_tokens,
                    row.cache_creation_tokens,
                    row.duration_ms,
                    row.tool_name,
                    row.tool_success.map(|b| b as i32),
                    row.prompt_length,
                    row.account_uuid,
                    row.timestamp,
                    row.raw_attributes,
                ],
            )
            .map_err(|e| format!("cannot insert otel_event: {e}"))?;
        Ok(())
    }

    pub fn otel_event_count(&self) -> Result<i64, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM otel_events", [], |r| r.get(0))
            .map_err(|e| format!("cannot count otel_events: {e}"))
    }

    pub fn otel_summary(&self) -> Result<OtelSummary, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*),
                    COALESCE(SUM(cost_usd), 0.0),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(duration_ms), 0),
                    COALESCE(SUM(CASE WHEN event_name = 'claude_code.tool_result' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN event_name = 'claude_code.api_request' THEN 1 ELSE 0 END), 0)
                 FROM otel_events",
                [],
                |row| {
                    Ok(OtelSummary {
                        event_count: row.get(0)?,
                        total_cost_usd: row.get(1)?,
                        total_input_tokens: row.get(2)?,
                        total_output_tokens: row.get(3)?,
                        total_cache_read_tokens: row.get(4)?,
                        total_duration_ms: row.get(5)?,
                        tool_calls: row.get(6)?,
                        api_requests: row.get(7)?,
                    })
                },
            )
            .map_err(|e| format!("cannot aggregate otel_events: {e}"))
    }
}
