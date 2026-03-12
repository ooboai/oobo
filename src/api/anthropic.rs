#![allow(dead_code)]

use super::UsageBucket;

const BASE_URL: &str = "https://api.anthropic.com/v1/organizations";

/// Fetch usage data from the Anthropic Admin API for the given time range.
/// Requires an Admin API key (sk-ant-admin...).
pub fn fetch_usage(
    api_key: &str,
    starting_at: &str,
    ending_at: &str,
) -> Result<Vec<UsageBucket>, String> {
    let url = format!(
        "{BASE_URL}/usage_report/messages?starting_at={starting_at}&ending_at={ending_at}&bucket_width=1d&group_by=model"
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(super::API_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .map_err(|e| format!("Anthropic API request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("Anthropic API returned {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("cannot parse Anthropic response: {e}"))?;

    parse_usage_response(&body)
}

/// Fetch cost data from the Anthropic Admin API.
pub fn fetch_costs(
    api_key: &str,
    starting_at: &str,
    ending_at: &str,
) -> Result<Vec<UsageBucket>, String> {
    let url = format!(
        "{BASE_URL}/cost_report?starting_at={starting_at}&ending_at={ending_at}&bucket_width=1d"
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(super::API_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .map_err(|e| format!("Anthropic cost API failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("Anthropic cost API returned {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("cannot parse Anthropic cost response: {e}"))?;

    parse_cost_response(&body)
}

fn parse_usage_response(body: &serde_json::Value) -> Result<Vec<UsageBucket>, String> {
    let mut buckets = Vec::new();

    let data = body.get("data").and_then(|d| d.as_array());
    let data = match data {
        Some(d) => d,
        None => return Ok(buckets),
    };

    for bucket in data {
        let date = bucket
            .get("bucket_start_time")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(10)
            .collect::<String>();

        let results = match bucket.get("results").and_then(|r| r.as_array()) {
            Some(r) => r,
            None => continue,
        };

        for result in results {
            let model = result
                .get("model")
                .and_then(|m| m.as_str())
                .map(String::from);

            let input = result
                .get("input_tokens")
                .or_else(|| result.get("uncached_input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = result
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_read = result
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_creation = result
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            if input == 0 && output == 0 && cache_read == 0 {
                continue;
            }

            buckets.push(UsageBucket {
                source: "anthropic".to_string(),
                date: date.clone(),
                model,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_creation_tokens: cache_creation,
                requests: 0,
            });
        }
    }

    Ok(buckets)
}

fn parse_cost_response(body: &serde_json::Value) -> Result<Vec<UsageBucket>, String> {
    let mut buckets = Vec::new();

    let data = body.get("data").and_then(|d| d.as_array());
    let data = match data {
        Some(d) => d,
        None => return Ok(buckets),
    };

    for bucket in data {
        let date = bucket
            .get("bucket_start_time")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(10)
            .collect::<String>();

        let results = match bucket.get("results").and_then(|r| r.as_array()) {
            Some(r) => r,
            None => continue,
        };

        for result in results {
            let cost = result
                .get("cost_cents")
                .and_then(|v| v.as_f64())
                .or_else(|| {
                    result
                        .get("amount")
                        .and_then(|a| a.get("value"))
                        .and_then(|v| v.as_f64())
                })
                .unwrap_or(0.0);

            if cost == 0.0 {
                continue;
            }

            buckets.push(UsageBucket {
                source: "anthropic".to_string(),
                date: date.clone(),
                model: None,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                requests: 0,
            });
        }
    }

    Ok(buckets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_usage_response() {
        let body: serde_json::Value = serde_json::json!({
            "data": [
                {
                    "bucket_start_time": "2026-03-01T00:00:00Z",
                    "results": [
                        {
                            "model": "claude-opus-4-5",
                            "input_tokens": 5000,
                            "output_tokens": 2000,
                            "cache_read_input_tokens": 1000,
                            "cache_creation_input_tokens": 500
                        }
                    ]
                }
            ]
        });

        let buckets = parse_usage_response(&body).unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].date, "2026-03-01");
        assert_eq!(buckets[0].model.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(buckets[0].input_tokens, 5000);
        assert_eq!(buckets[0].output_tokens, 2000);
    }

    #[test]
    fn test_parse_cost_response() {
        let body: serde_json::Value = serde_json::json!({
            "data": [
                {
                    "bucket_start_time": "2026-03-01T00:00:00Z",
                    "results": [
                        { "cost_cents": 150.0 }
                    ]
                }
            ]
        });

        let buckets = parse_cost_response(&body).unwrap();
        assert_eq!(buckets.len(), 1);
    }
}
