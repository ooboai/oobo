#![allow(dead_code)]

use rusqlite::{params, OptionalExtension};

use super::Db;

#[derive(Debug, Clone, Default)]
pub struct StatsRow {
    pub session_id: String,
    pub source: String,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub is_estimated: bool,
    pub token_source: String,
    pub duration_secs: Option<i64>,
    pub files_touched: Vec<String>,
    pub tool_call_count: i32,
    pub computed_at: i64,
}

impl StatsRow {
    /// Returns `true` when the session was updated after these stats were computed.
    /// Mirrors the SQL staleness check in [`UPDATED_AT_EPOCH_SECS_SQL`].
    pub fn is_stale(&self, updated_at: Option<i64>) -> bool {
        if let Some(updated) = updated_at {
            let updated_secs = crate::utils::to_epoch_secs(updated);
            updated_secs > self.computed_at
        } else {
            false
        }
    }
}

/// SQL expression that normalizes `s.updated_at` (which may be in seconds,
/// milliseconds, or microseconds) to epoch seconds.
/// Must stay in sync with [`crate::utils::to_epoch_secs`] and [`StatsRow::is_stale`].
pub const UPDATED_AT_EPOCH_SECS_SQL: &str =
    "CASE WHEN s.updated_at >= 1000000000000000 THEN s.updated_at / 1000000 \
          WHEN s.updated_at >= 1000000000000 THEN s.updated_at / 1000 \
          ELSE s.updated_at END";

/// Aggregated statistics across multiple sessions.
#[derive(Debug, Clone, Default)]
pub struct AggregateStats {
    pub session_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_duration_secs: i64,
}

/// Per-model aggregated statistics.
#[derive(Debug, Clone, Default)]
pub struct ModelStats {
    pub model: String,
    pub session_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_duration_secs: i64,
    pub pct_of_total_output: f64,
}

impl Db {
    pub fn upsert_stats(&self, stats: &StatsRow) -> Result<(), String> {
        let files_json =
            serde_json::to_string(&stats.files_touched).unwrap_or_else(|_| "[]".to_string());

        self.conn
            .execute(
                "INSERT INTO session_stats (session_id, source, model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, is_estimated, token_source, duration_secs, files_touched, tool_call_count, computed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(session_id, source) DO UPDATE SET
                     model = COALESCE(excluded.model, session_stats.model),
                     input_tokens = excluded.input_tokens,
                     output_tokens = excluded.output_tokens,
                     cache_read_tokens = excluded.cache_read_tokens,
                     cache_creation_tokens = excluded.cache_creation_tokens,
                     is_estimated = excluded.is_estimated,
                     token_source = excluded.token_source,
                     duration_secs = excluded.duration_secs,
                     files_touched = excluded.files_touched,
                     tool_call_count = excluded.tool_call_count,
                     computed_at = excluded.computed_at",
                params![
                    stats.session_id,
                    stats.source,
                    stats.model,
                    stats.input_tokens,
                    stats.output_tokens,
                    stats.cache_read_tokens,
                    stats.cache_creation_tokens,
                    stats.is_estimated as i32,
                    stats.token_source,
                    stats.duration_secs,
                    files_json,
                    stats.tool_call_count,
                    stats.computed_at,
                ],
            )
            .map_err(|e| format!("cannot upsert stats: {e}"))?;
        Ok(())
    }

    pub fn get_stats(&self, session_id: &str, source: &str) -> Result<Option<StatsRow>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, source, model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, is_estimated, token_source, duration_secs, files_touched, tool_call_count, computed_at
             FROM session_stats WHERE session_id = ?1 AND source = ?2"
        ).map_err(|e| format!("cannot prepare: {e}"))?;

        stmt.query_row(params![session_id, source], |row| Ok(row_to_stats(row)))
            .optional()
            .map_err(|e| format!("cannot query stats: {e}"))
    }

    /// Aggregate token/cost stats for all sessions of a given project.
    pub fn aggregate_stats_by_project(&self, project_id: &str) -> Result<AggregateStats, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(duration_secs), 0)
                 FROM session_stats ss
                 JOIN sessions s ON ss.session_id = s.id AND ss.source = s.source
                 WHERE s.project_id = ?1",
                params![project_id],
                |row| {
                    Ok(AggregateStats {
                        session_count: row.get(0)?,
                        total_input_tokens: row.get(1)?,
                        total_output_tokens: row.get(2)?,
                        total_duration_secs: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("cannot aggregate stats: {e}"))
    }

    /// Aggregate token/cost stats for a specific tool across all projects.
    pub fn aggregate_stats_by_tool(&self, source: &str) -> Result<AggregateStats, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(duration_secs), 0)
                 FROM session_stats WHERE source = ?1",
                params![source],
                |row| {
                    Ok(AggregateStats {
                        session_count: row.get(0)?,
                        total_input_tokens: row.get(1)?,
                        total_output_tokens: row.get(2)?,
                        total_duration_secs: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("cannot aggregate stats: {e}"))
    }

    /// Aggregate stats grouped by tool (source), returning (tool_name, stats) pairs.
    pub fn aggregate_stats_per_tool(&self) -> Result<Vec<(String, AggregateStats)>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT source, COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(duration_secs), 0)
             FROM session_stats GROUP BY source ORDER BY SUM(output_tokens) DESC",
        ).map_err(|e| format!("cannot prepare per-tool query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AggregateStats {
                        session_count: row.get(1)?,
                        total_input_tokens: row.get(2)?,
                        total_output_tokens: row.get(3)?,
                        total_duration_secs: row.get(4)?,
                    },
                ))
            })
            .map_err(|e| format!("cannot query per-tool stats: {e}"))?;

        super::collect_rows(rows)
    }

    /// Aggregate stats per project, returning (project_name, stats) sorted by output tokens desc.
    pub fn aggregate_stats_top_projects(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, AggregateStats)>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name, COUNT(*), COALESCE(SUM(ss.input_tokens), 0), COALESCE(SUM(ss.output_tokens), 0), COALESCE(SUM(ss.duration_secs), 0)
             FROM session_stats ss
             JOIN sessions s ON s.id = ss.session_id AND s.source = ss.source
             JOIN projects p ON p.id = s.project_id
             GROUP BY p.id
             ORDER BY SUM(ss.output_tokens) DESC
             LIMIT ?1",
        ).map_err(|e| format!("cannot prepare top-projects query: {e}"))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AggregateStats {
                        session_count: row.get(1)?,
                        total_input_tokens: row.get(2)?,
                        total_output_tokens: row.get(3)?,
                        total_duration_secs: row.get(4)?,
                    },
                ))
            })
            .map_err(|e| format!("cannot query top-projects: {e}"))?;

        super::collect_rows(rows)
    }

    /// Daily token/cost breakdown, joining through sessions for created_at.
    /// Returns (date_string, stats) pairs sorted newest first, limited to `days`.
    pub fn daily_stats(&self, days: usize) -> Result<Vec<(String, AggregateStats)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT date(s.created_at, 'unixepoch') AS day,
                    COUNT(*),
                    COALESCE(SUM(ss.input_tokens), 0),
                    COALESCE(SUM(ss.output_tokens), 0),
                    COALESCE(SUM(ss.duration_secs), 0)
             FROM session_stats ss
             JOIN sessions s ON s.id = ss.session_id AND s.source = ss.source
             WHERE s.created_at IS NOT NULL
             GROUP BY day
             HAVING day IS NOT NULL
             ORDER BY day DESC
             LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare daily query: {e}"))?;

        let rows = stmt
            .query_map(params![days as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AggregateStats {
                        session_count: row.get(1)?,
                        total_input_tokens: row.get(2)?,
                        total_output_tokens: row.get(3)?,
                        total_duration_secs: row.get(4)?,
                    },
                ))
            })
            .map_err(|e| format!("daily query error: {e}"))?;

        super::collect_rows(rows)
    }

    /// Aggregate token/cost stats globally.
    pub fn aggregate_stats_global(&self) -> Result<AggregateStats, String> {
        self.aggregate_stats_global_since(None)
    }

    /// Aggregate stats globally, optionally filtered by a cutoff timestamp.
    pub fn aggregate_stats_global_since(
        &self,
        since: Option<i64>,
    ) -> Result<AggregateStats, String> {
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(ts) = since {
            (
                "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(duration_secs), 0)
                 FROM session_stats ss
                 JOIN sessions s ON s.id = ss.session_id AND s.source = ss.source
                 WHERE s.created_at >= ?1",
                vec![Box::new(ts)],
            )
        } else {
            (
                "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(duration_secs), 0)
                 FROM session_stats",
                vec![],
            )
        };
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        self.conn
            .query_row(sql, param_refs.as_slice(), |row| {
                Ok(AggregateStats {
                    session_count: row.get(0)?,
                    total_input_tokens: row.get(1)?,
                    total_output_tokens: row.get(2)?,
                    total_duration_secs: row.get(3)?,
                })
            })
            .map_err(|e| format!("cannot aggregate stats: {e}"))
    }

    /// Weekly aggregated stats for the last N weeks.
    pub fn weekly_stats(&self, weeks: usize) -> Result<Vec<(String, AggregateStats)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT strftime('%Y-W%W', s.created_at, 'unixepoch') AS week,
                    COUNT(*),
                    COALESCE(SUM(ss.input_tokens), 0),
                    COALESCE(SUM(ss.output_tokens), 0),
                    COALESCE(SUM(ss.duration_secs), 0)
             FROM session_stats ss
             JOIN sessions s ON s.id = ss.session_id AND s.source = ss.source
             WHERE s.created_at IS NOT NULL
             GROUP BY week
             HAVING week IS NOT NULL
             ORDER BY week DESC
             LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare weekly query: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params![weeks as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AggregateStats {
                        session_count: row.get(1)?,
                        total_input_tokens: row.get(2)?,
                        total_output_tokens: row.get(3)?,
                        total_duration_secs: row.get(4)?,
                    },
                ))
            })
            .map_err(|e| format!("weekly query error: {e}"))?;

        super::collect_rows(rows)
    }

    /// Monthly aggregated stats for the last N months.
    pub fn monthly_stats(&self, months: usize) -> Result<Vec<(String, AggregateStats)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT strftime('%Y-%m', s.created_at, 'unixepoch') AS month,
                    COUNT(*),
                    COALESCE(SUM(ss.input_tokens), 0),
                    COALESCE(SUM(ss.output_tokens), 0),
                    COALESCE(SUM(ss.duration_secs), 0)
             FROM session_stats ss
             JOIN sessions s ON s.id = ss.session_id AND s.source = ss.source
             WHERE s.created_at IS NOT NULL
             GROUP BY month
             HAVING month IS NOT NULL
             ORDER BY month DESC
             LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare monthly query: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params![months as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AggregateStats {
                        session_count: row.get(1)?,
                        total_input_tokens: row.get(2)?,
                        total_output_tokens: row.get(3)?,
                        total_duration_secs: row.get(4)?,
                    },
                ))
            })
            .map_err(|e| format!("monthly query error: {e}"))?;

        super::collect_rows(rows)
    }

    /// Daily token/cost breakdown per tool.
    pub fn daily_stats_by_tool(
        &self,
        days: usize,
    ) -> Result<Vec<(String, String, AggregateStats)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT date(s.created_at, 'unixepoch') AS day,
                    ss.source,
                    COUNT(*),
                    COALESCE(SUM(ss.input_tokens), 0),
                    COALESCE(SUM(ss.output_tokens), 0),
                    COALESCE(SUM(ss.duration_secs), 0)
             FROM session_stats ss
             JOIN sessions s ON s.id = ss.session_id AND s.source = ss.source
             WHERE s.created_at IS NOT NULL
             GROUP BY day, ss.source
             HAVING day IS NOT NULL
             ORDER BY day DESC
             LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare daily-by-tool query: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params![days as i64 * 10], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    AggregateStats {
                        session_count: row.get(2)?,
                        total_input_tokens: row.get(3)?,
                        total_output_tokens: row.get(4)?,
                        total_duration_secs: row.get(5)?,
                    },
                ))
            })
            .map_err(|e| format!("daily-by-tool query error: {e}"))?;

        super::collect_rows(rows)
    }

    /// Aggregate stats grouped by model.
    pub fn aggregate_stats_by_model(&self) -> Result<Vec<ModelStats>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT model, COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(duration_secs), 0)
             FROM session_stats
             WHERE model IS NOT NULL AND model != ''
             GROUP BY model
             ORDER BY SUM(output_tokens) DESC",
            )
            .map_err(|e| format!("cannot prepare model query: {e}"))?;

        let total_output: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(output_tokens), 0) FROM session_stats WHERE model IS NOT NULL AND model != ''",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let rows = stmt
            .query_map([], |row| {
                let output: i64 = row.get(3)?;
                let pct_of_total = if total_output > 0 {
                    100.0 * output as f64 / total_output as f64
                } else {
                    0.0
                };
                Ok(ModelStats {
                    model: row.get(0)?,
                    session_count: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: output,
                    total_duration_secs: row.get(4)?,
                    pct_of_total_output: pct_of_total,
                })
            })
            .map_err(|e| format!("model query error: {e}"))?;

        super::collect_rows(rows)
    }

    /// Fetch all session stats as a map keyed by (session_id, source).
    /// Pass an empty slice to load all stats.
    pub fn get_stats_bulk(
        &self,
        _keys: &[(String, String)],
    ) -> Result<std::collections::HashMap<(String, String), StatsRow>, String> {
        // NOTE: _keys is currently unused — always loads all stats.
        // This is intentional: callers pass &[] to mean "get everything."
        // If filtering is needed later, add a WHERE clause with key matching.
        use std::collections::HashMap;
        let mut map = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT session_id, source, model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, is_estimated, token_source, duration_secs, files_touched, tool_call_count, computed_at
             FROM session_stats"
        ).map_err(|e| format!("cannot prepare bulk stats: {e}"))?;

        let rows = stmt
            .query_map([], |row| Ok(row_to_stats(row)))
            .map_err(|e| format!("cannot query bulk stats: {e}"))?;

        for s in rows.flatten() {
            let key = (s.session_id.clone(), s.source.clone());
            map.insert(key, s);
        }
        Ok(map)
    }
}

fn row_to_stats(row: &rusqlite::Row) -> StatsRow {
    let files_json: String = row.get(10).unwrap_or_default();
    let files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();
    let is_est: i32 = row.get(7).unwrap_or(0);

    StatsRow {
        session_id: row.get(0).unwrap_or_default(),
        source: row.get(1).unwrap_or_default(),
        model: row.get(2).unwrap_or_default(),
        input_tokens: row.get(3).unwrap_or_default(),
        output_tokens: row.get(4).unwrap_or_default(),
        cache_read_tokens: row.get(5).unwrap_or_default(),
        cache_creation_tokens: row.get(6).unwrap_or_default(),
        is_estimated: is_est != 0,
        token_source: row.get(8).unwrap_or_else(|_| "native".to_string()),
        duration_secs: row.get(9).unwrap_or_default(),
        files_touched: files,
        tool_call_count: row.get(11).unwrap_or(0),
        computed_at: row.get(12).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::projects::ProjectRow;
    use crate::db::sessions::SessionRow;

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_project(&ProjectRow {
            id: "proj".into(),
            path: "/proj".into(),
            name: "proj".into(),
            git_remote: None,
            initial_commit_sha: None,
            historical_paths: Vec::new(),
            discovered_at: 1000,
            last_seen_at: 1000,
            last_scanned_at: 0,
            tools: vec![],
        })
        .unwrap();
        db.upsert_session(&SessionRow {
            id: "s1".into(),
            source: "claude".into(),
            project_id: "proj".into(),
            name: Some("test".into()),
            mode: None,
            model: None,
            created_at: Some(1000),
            updated_at: Some(2000),
            message_count: 5,
            first_message: None,
            indexed_at: 3000,
        })
        .unwrap();
        db
    }

    fn sample_stats() -> StatsRow {
        StatsRow {
            session_id: "s1".into(),
            source: "claude".into(),
            model: Some("claude-opus-4-5".into()),
            input_tokens: Some(15000),
            output_tokens: Some(8000),
            cache_read_tokens: Some(10000),
            cache_creation_tokens: Some(2000),
            is_estimated: false,
            token_source: "native".into(),
            duration_secs: Some(120),
            files_touched: vec!["src/main.rs".into()],
            tool_call_count: 3,
            computed_at: 5000,
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        let found = db.get_stats("s1", "claude").unwrap().unwrap();
        assert_eq!(found.input_tokens, Some(15000));
        assert!(!found.is_estimated);
        assert_eq!(found.token_source, "native");
        assert_eq!(found.files_touched, vec!["src/main.rs"]);
    }

    #[test]
    fn test_aggregate_by_project() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        let agg = db.aggregate_stats_by_project("proj").unwrap();
        assert_eq!(agg.session_count, 1);
        assert_eq!(agg.total_input_tokens, 15000);
        assert_eq!(agg.total_output_tokens, 8000);
    }

    #[test]
    fn test_aggregate_by_tool() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        let agg = db.aggregate_stats_by_tool("claude").unwrap();
        assert_eq!(agg.session_count, 1);

        let agg = db.aggregate_stats_by_tool("cursor").unwrap();
        assert_eq!(agg.session_count, 0);
    }

    #[test]
    fn test_aggregate_global() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        let agg = db.aggregate_stats_global().unwrap();
        assert_eq!(agg.session_count, 1);
    }

    #[test]
    fn test_get_nonexistent() {
        let db = test_db();
        let found = db.get_stats("nope", "claude").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_upsert_overwrites_on_conflict() {
        let db = test_db();
        let mut stats = sample_stats();
        db.upsert_stats(&stats).unwrap();

        stats.input_tokens = Some(99999);
        stats.tool_call_count = 10;
        db.upsert_stats(&stats).unwrap();

        let found = db.get_stats("s1", "claude").unwrap().unwrap();
        assert_eq!(found.input_tokens, Some(99999));
        assert_eq!(found.tool_call_count, 10);
    }

    #[test]
    fn test_get_stats_bulk() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        let keys = vec![("s1".to_string(), "claude".to_string())];
        let map = db.get_stats_bulk(&keys).unwrap();
        assert_eq!(map.len(), 1);
        let entry = map.get(&("s1".to_string(), "claude".to_string())).unwrap();
        assert_eq!(entry.input_tokens, Some(15000));
    }

    #[test]
    fn test_aggregate_per_tool_multiple_sources() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        db.upsert_session(&SessionRow {
            id: "s2".into(),
            source: "cursor".into(),
            project_id: "proj".into(),
            name: Some("cursor session".into()),
            mode: None,
            model: None,
            created_at: Some(1000),
            updated_at: Some(2000),
            message_count: 3,
            first_message: None,
            indexed_at: 3000,
        })
        .unwrap();

        db.upsert_stats(&StatsRow {
            session_id: "s2".into(),
            source: "cursor".into(),
            model: Some("gpt-4o".into()),
            input_tokens: Some(5000),
            output_tokens: Some(2000),
            ..Default::default()
        })
        .unwrap();

        let per_tool = db.aggregate_stats_per_tool().unwrap();
        assert_eq!(per_tool.len(), 2);
        let sources: Vec<&str> = per_tool.iter().map(|(s, _)| s.as_str()).collect();
        assert!(sources.contains(&"claude"));
        assert!(sources.contains(&"cursor"));
    }

    #[test]
    fn test_aggregate_by_model() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        let models = db.aggregate_stats_by_model().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "claude-opus-4-5");
        assert_eq!(models[0].output_tokens, 8000);
        assert!((models[0].pct_of_total_output - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_aggregate_global_empty() {
        let db = test_db();
        let agg = db.aggregate_stats_global().unwrap();
        assert_eq!(agg.session_count, 0);
        assert_eq!(agg.total_input_tokens, 0);
    }

    #[test]
    fn test_daily_stats() {
        let db = test_db();
        db.upsert_stats(&sample_stats()).unwrap();

        let daily = db.daily_stats(30).unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].1.total_input_tokens, 15000);
    }

    #[test]
    fn test_is_stale_newer_updated_at() {
        let stats = StatsRow {
            computed_at: 3000,
            ..Default::default()
        };
        assert!(stats.is_stale(Some(5000)));
    }

    #[test]
    fn test_is_stale_older_updated_at() {
        let stats = StatsRow {
            computed_at: 3000,
            ..Default::default()
        };
        assert!(!stats.is_stale(Some(2000)));
    }

    #[test]
    fn test_is_stale_equal_timestamps() {
        let stats = StatsRow {
            computed_at: 3000,
            ..Default::default()
        };
        assert!(!stats.is_stale(Some(3000)));
    }

    #[test]
    fn test_is_stale_none_updated_at() {
        let stats = StatsRow {
            computed_at: 3000,
            ..Default::default()
        };
        assert!(!stats.is_stale(None));
    }

    #[test]
    fn test_is_stale_millis_normalization() {
        let stats = StatsRow {
            computed_at: 1700000000,
            ..Default::default()
        };
        // 1700000001000 ms → 1700000001 secs, which is > 1700000000
        assert!(stats.is_stale(Some(1700000001000)));
    }
}
