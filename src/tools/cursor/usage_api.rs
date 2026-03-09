#![allow(dead_code)]

use std::path::PathBuf;

/// Extract the user's Cursor session JWT from the local state.vscdb SQLite database.
/// This is an undocumented internal token that Cursor uses for its dashboard API.
pub fn extract_jwt() -> Option<String> {
    let db_path = state_vscdb_path()?;
    if !db_path.exists() {
        return None;
    }

    let conn = crate::utils::open_db_readonly(&db_path).ok()?;

    // Cursor stores auth tokens in state.vscdb under various keys
    let token_keys = ["cursorAuth/accessToken", "cursorAuth/cachedSignUpType"];

    for key in &token_keys {
        let result: Result<String, _> =
            conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| {
                row.get(0)
            });
        if let Ok(token) = result {
            let token = token.trim().trim_matches('"').to_string();
            if !token.is_empty() && token.len() > 20 {
                return Some(token);
            }
        }
    }

    None
}

/// Fetch monthly usage data from Cursor's IDE backend API using the local JWT.
/// Returns aggregate request/token counts for the current billing period.
pub fn fetch_usage(jwt: &str) -> Result<CursorUsage, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get("https://api2.cursor.sh/auth/usage")
        .bearer_auth(jwt)
        .send()
        .map_err(|e| format!("Cursor API request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Cursor API returned {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("cannot parse Cursor API response: {e}"))?;

    parse_usage_v2(&body)
}

/// Fetch subscription/profile info from Cursor's backend.
pub fn fetch_profile(jwt: &str) -> Result<CursorProfile, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get("https://api2.cursor.sh/auth/full_stripe_profile")
        .bearer_auth(jwt)
        .send()
        .map_err(|e| format!("Cursor profile request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Cursor profile API returned {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("cannot parse profile response: {e}"))?;

    Ok(CursorProfile {
        membership_type: body
            .get("membershipType")
            .and_then(|v| v.as_str())
            .map(String::from),
        subscription_status: body
            .get("subscriptionStatus")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// Fetch detailed usage events from the Cursor Enterprise Admin API.
pub fn fetch_enterprise_usage(api_key: &str) -> Result<Vec<UsageEvent>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(crate::api::API_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .post("https://api.cursor.com/teams/filtered-usage-events")
        .basic_auth(api_key, Option::<&str>::None)
        .json(&serde_json::json!({}))
        .send()
        .map_err(|e| format!("Cursor Enterprise API request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Cursor Enterprise API returned {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("cannot parse Enterprise API response: {e}"))?;

    parse_enterprise_response(&body)
}

#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_cents: f64,
    pub timestamp: Option<String>,
}

/// Monthly aggregate usage from api2.cursor.sh/auth/usage
#[derive(Debug, Clone)]
pub struct CursorUsage {
    pub start_of_month: Option<String>,
    pub buckets: Vec<UsageBucket>,
}

#[derive(Debug, Clone)]
pub struct UsageBucket {
    pub model_category: String,
    pub num_requests: u64,
    pub num_requests_total: u64,
    pub num_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct CursorProfile {
    pub membership_type: Option<String>,
    pub subscription_status: Option<String>,
}

fn parse_usage_v2(body: &serde_json::Value) -> Result<CursorUsage, String> {
    let start_of_month = body
        .get("startOfMonth")
        .and_then(|v| v.as_str())
        .map(String::from);

    let mut buckets = Vec::new();

    if let Some(obj) = body.as_object() {
        for (key, val) in obj {
            if key == "startOfMonth" {
                continue;
            }
            let num_requests = val.get("numRequests").and_then(|v| v.as_u64()).unwrap_or(0);
            let num_requests_total = val
                .get("numRequestsTotal")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let num_tokens = val.get("numTokens").and_then(|v| v.as_u64()).unwrap_or(0);
            buckets.push(UsageBucket {
                model_category: key.clone(),
                num_requests,
                num_requests_total,
                num_tokens,
            });
        }
    }

    Ok(CursorUsage {
        start_of_month,
        buckets,
    })
}

fn parse_enterprise_response(body: &serde_json::Value) -> Result<Vec<UsageEvent>, String> {
    let mut events = Vec::new();

    let items = body
        .get("data")
        .or_else(|| body.get("events"))
        .and_then(|d| d.as_array());

    if let Some(items) = items {
        for item in items {
            let model = item.get("model").and_then(|m| m.as_str()).map(String::from);
            let timestamp = item
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(String::from);

            let (input, output, cache_read, cache_write, cost) =
                if let Some(tu) = item.get("tokenUsage") {
                    (
                        tu.get("inputTokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        tu.get("outputTokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        tu.get("cacheReadTokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        tu.get("cacheWriteTokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        tu.get("totalCents").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    )
                } else {
                    (0, 0, 0, 0, 0.0)
                };

            let charged = item
                .get("chargedCents")
                .and_then(|v| v.as_f64())
                .unwrap_or(cost);

            events.push(UsageEvent {
                model,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
                cost_cents: charged,
                timestamp,
            });
        }
    }

    Ok(events)
}

fn parse_single_event(item: &serde_json::Value) -> Option<UsageEvent> {
    let model = item.get("model").and_then(|m| m.as_str()).map(String::from);
    let input = item
        .get("inputTokens")
        .or_else(|| item.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = item
        .get("outputTokens")
        .or_else(|| item.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = item
        .get("cacheReadTokens")
        .or_else(|| item.get("cache_read_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_write = item
        .get("cacheWriteTokens")
        .or_else(|| item.get("cache_write_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cost = item
        .get("costCents")
        .or_else(|| item.get("cost_cents"))
        .or_else(|| item.get("chargedCents"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let timestamp = item
        .get("timestamp")
        .or_else(|| item.get("createdAt"))
        .and_then(|t| t.as_str())
        .map(String::from);

    if input == 0 && output == 0 && model.is_none() {
        return None;
    }

    Some(UsageEvent {
        model,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        cost_cents: cost,
        timestamp,
    })
}

fn state_vscdb_path() -> Option<PathBuf> {
    super::state_vscdb_path()
}

/// Aggregate usage events into a summary for the analytics pipeline.
pub fn aggregate_events(events: &[UsageEvent]) -> crate::analytics::NativeStats {
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cache_read = 0u64;
    let mut total_cache_write = 0u64;
    let mut total_cost_cents = 0.0f64;
    let mut model: Option<String> = None;

    for evt in events {
        total_input += evt.input_tokens;
        total_output += evt.output_tokens;
        total_cache_read += evt.cache_read_tokens;
        total_cache_write += evt.cache_write_tokens;
        total_cost_cents += evt.cost_cents;
        if model.is_none() {
            model = evt.model.clone();
        }
    }

    crate::analytics::NativeStats {
        model,
        input_tokens: if total_input > 0 {
            Some(total_input)
        } else {
            None
        },
        output_tokens: if total_output > 0 {
            Some(total_output)
        } else {
            None
        },
        cache_read_tokens: if total_cache_read > 0 {
            Some(total_cache_read)
        } else {
            None
        },
        cache_creation_tokens: if total_cache_write > 0 {
            Some(total_cache_write)
        } else {
            None
        },
        total_cost_usd: if total_cost_cents > 0.0 {
            Some(total_cost_cents / 100.0)
        } else {
            None
        },
        ..Default::default()
    }
}
