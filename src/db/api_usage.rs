#![allow(dead_code)]

use rusqlite::params;

use super::Db;
use crate::api::UsageBucket;

impl Db {
    /// Upsert a batch of API usage buckets into the database.
    pub fn upsert_api_usage(&self, buckets: &[UsageBucket]) -> Result<usize, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut count = 0;
        for b in buckets {
            let model_key = b.model.as_deref().unwrap_or("");
            self.conn
                .execute(
                    "INSERT INTO api_usage (source, date, model, input_tokens, output_tokens,
                         cache_read_tokens, cache_creation_tokens, cost_usd, requests, fetched_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(source, date, model)
                     DO UPDATE SET
                         input_tokens = excluded.input_tokens,
                         output_tokens = excluded.output_tokens,
                         cache_read_tokens = excluded.cache_read_tokens,
                         cache_creation_tokens = excluded.cache_creation_tokens,
                         cost_usd = excluded.cost_usd,
                         requests = excluded.requests,
                         fetched_at = excluded.fetched_at",
                    params![
                        b.source,
                        b.date,
                        model_key,
                        b.input_tokens as i64,
                        b.output_tokens as i64,
                        b.cache_read_tokens as i64,
                        b.cache_creation_tokens as i64,
                        b.cost_usd,
                        b.requests as i64,
                        now,
                    ],
                )
                .map_err(|e| format!("cannot upsert api_usage: {e}"))?;
            count += 1;
        }

        Ok(count)
    }

    /// Get aggregated API usage for a given source.
    pub fn api_usage_summary(&self, source: &str) -> Result<ApiUsageSummary, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COALESCE(SUM(requests), 0),
                    COUNT(DISTINCT date),
                    MAX(fetched_at)
                 FROM api_usage WHERE source = ?1",
            )
            .map_err(|e| format!("cannot prepare api_usage query: {e}"))?;

        let summary = stmt
            .query_row(params![source], |row| {
                Ok(ApiUsageSummary {
                    input_tokens: row.get::<_, i64>(0)? as u64,
                    output_tokens: row.get::<_, i64>(1)? as u64,
                    cache_read_tokens: row.get::<_, i64>(2)? as u64,
                    cache_creation_tokens: row.get::<_, i64>(3)? as u64,
                    cost_usd: row.get(4)?,
                    requests: row.get::<_, i64>(5)? as u64,
                    days: row.get::<_, i64>(6)? as u64,
                    last_fetched_at: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                })
            })
            .map_err(|e| format!("cannot query api_usage: {e}"))?;

        Ok(summary)
    }

    /// Get aggregated API usage across all sources.
    pub fn api_usage_totals(&self) -> Result<Vec<(String, ApiUsageSummary)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    source,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COALESCE(SUM(requests), 0),
                    COUNT(DISTINCT date),
                    MAX(fetched_at)
                 FROM api_usage GROUP BY source",
            )
            .map_err(|e| format!("cannot prepare api_usage totals query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                let source: String = row.get(0)?;
                let summary = ApiUsageSummary {
                    input_tokens: row.get::<_, i64>(1)? as u64,
                    output_tokens: row.get::<_, i64>(2)? as u64,
                    cache_read_tokens: row.get::<_, i64>(3)? as u64,
                    cache_creation_tokens: row.get::<_, i64>(4)? as u64,
                    cost_usd: row.get(5)?,
                    requests: row.get::<_, i64>(6)? as u64,
                    days: row.get::<_, i64>(7)? as u64,
                    last_fetched_at: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                };
                Ok((source, summary))
            })
            .map_err(|e| format!("cannot query api_usage totals: {e}"))?;

        super::collect_rows(rows)
    }
}

#[derive(Debug, Default)]
pub struct ApiUsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
    pub requests: u64,
    pub days: u64,
    pub last_fetched_at: i64,
}

impl ApiUsageSummary {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens
    }

    pub fn has_data(&self) -> bool {
        self.total_tokens() > 0 || self.cost_usd > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_query() {
        let db = Db::open_in_memory().unwrap();

        let buckets = vec![
            UsageBucket {
                source: "anthropic".to_string(),
                date: "2026-03-01".to_string(),
                model: Some("claude-opus-4-5".to_string()),
                input_tokens: 5000,
                output_tokens: 2000,
                cache_read_tokens: 1000,
                cache_creation_tokens: 500,
                cost_usd: 0.15,
                requests: 10,
            },
            UsageBucket {
                source: "anthropic".to_string(),
                date: "2026-03-02".to_string(),
                model: Some("claude-sonnet-4".to_string()),
                input_tokens: 3000,
                output_tokens: 1000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: 0.08,
                requests: 5,
            },
        ];

        let count = db.upsert_api_usage(&buckets).unwrap();
        assert_eq!(count, 2);

        let summary = db.api_usage_summary("anthropic").unwrap();
        assert_eq!(summary.input_tokens, 8000);
        assert_eq!(summary.output_tokens, 3000);
        assert_eq!(summary.days, 2);
        assert!(summary.has_data());
    }

    #[test]
    fn test_upsert_idempotent() {
        let db = Db::open_in_memory().unwrap();

        let bucket = UsageBucket {
            source: "openai".to_string(),
            date: "2026-03-01".to_string(),
            model: Some("gpt-5.2".to_string()),
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cost_usd: 0.05,
            requests: 3,
        };

        db.upsert_api_usage(std::slice::from_ref(&bucket)).unwrap();
        let updated = UsageBucket {
            input_tokens: 2000,
            ..bucket
        };
        db.upsert_api_usage(&[updated]).unwrap();

        let summary = db.api_usage_summary("openai").unwrap();
        assert_eq!(summary.input_tokens, 2000);
    }
}
