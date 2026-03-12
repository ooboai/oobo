pub mod anthropic;
pub mod openai;

use std::time::Duration;

use chrono::Utc;

pub const API_TIMEOUT: Duration = Duration::from_secs(15);

/// A bucket of API usage data for a single time period.
#[derive(Debug, Clone)]
pub struct UsageBucket {
    pub source: String,
    pub date: String,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub requests: u64,
}

/// Fetch all available API usage data based on configured keys.
/// Returns a vec of usage buckets and a summary of what was fetched.
pub fn fetch_all(cfg: &crate::config::Config) -> Vec<FetchResult> {
    let mut results = Vec::new();

    if !cfg.claude.api_key.is_empty() {
        results.push(fetch_anthropic(&cfg.claude.api_key));
    }

    if !cfg.codex.api_key.is_empty() {
        results.push(fetch_openai(&cfg.codex.api_key));
    }

    if cfg.cursor.enabled {
        if let Some(r) = fetch_cursor_usage(&cfg.cursor.api_key) {
            results.push(r);
        }
    }

    results
}

pub struct FetchResult {
    pub source: &'static str,
    pub buckets: Vec<UsageBucket>,
    pub error: Option<String>,
}

fn fetch_anthropic(api_key: &str) -> FetchResult {
    let now = Utc::now();
    let start = now - chrono::Duration::days(30);
    let start_str = start.format("%Y-%m-%dT00:00:00Z").to_string();
    let end_str = now.format("%Y-%m-%dT23:59:59Z").to_string();

    match anthropic::fetch_usage(api_key, &start_str, &end_str) {
        Ok(buckets) => FetchResult {
            source: "anthropic",
            buckets,
            error: None,
        },
        Err(e) => FetchResult {
            source: "anthropic",
            buckets: Vec::new(),
            error: Some(e),
        },
    }
}

fn fetch_cursor_usage(api_key: &str) -> Option<FetchResult> {
    let jwt = if !api_key.is_empty() {
        Some(api_key.to_string())
    } else {
        crate::tools::cursor::usage_api::extract_jwt()
    };

    let jwt = jwt?;

    let usage = match crate::tools::cursor::usage_api::fetch_usage(&jwt) {
        Ok(u) => u,
        Err(_) => return None,
    };

    if usage.buckets.is_empty() {
        return None;
    }

    let date = usage
        .start_of_month
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(10)
        .collect::<String>();

    let buckets: Vec<UsageBucket> = usage
        .buckets
        .into_iter()
        .filter(|b| b.num_requests_total > 0 || b.num_tokens > 0)
        .map(|b| UsageBucket {
            source: "cursor".to_string(),
            date: date.clone(),
            model: Some(b.model_category),
            input_tokens: b.num_tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            requests: b.num_requests_total,
        })
        .collect();

    if buckets.is_empty() {
        return None;
    }

    Some(FetchResult {
        source: "cursor",
        buckets,
        error: None,
    })
}

fn fetch_openai(api_key: &str) -> FetchResult {
    let now = Utc::now();
    let start = now - chrono::Duration::days(30);
    let start_ts = start.timestamp();

    match openai::fetch_costs(api_key, start_ts) {
        Ok(buckets) => FetchResult {
            source: "openai",
            buckets,
            error: None,
        },
        Err(e) => FetchResult {
            source: "openai",
            buckets: Vec::new(),
            error: Some(e),
        },
    }
}
