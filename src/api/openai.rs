#![allow(dead_code)]

use super::UsageBucket;

const BASE_URL: &str = "https://api.openai.com/v1/organization";

/// Fetch completions usage from the OpenAI Usage API.
/// Requires an Admin API key.
pub fn fetch_completions(api_key: &str, start_time: i64) -> Result<Vec<UsageBucket>, String> {
    let url = format!(
        "{BASE_URL}/usage/completions?start_time={start_time}&bucket_width=1d&group_by[]=model"
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(super::API_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .map_err(|e| format!("OpenAI Usage API failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("OpenAI Usage API returned {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("cannot parse OpenAI usage response: {e}"))?;

    parse_completions_response(&body)
}

/// Fetch cost data from the OpenAI Costs API.
pub fn fetch_costs(api_key: &str, start_time: i64) -> Result<Vec<UsageBucket>, String> {
    let url =
        format!("{BASE_URL}/costs?start_time={start_time}&bucket_width=1d&group_by[]=line_item");

    let client = reqwest::blocking::Client::builder()
        .timeout(super::API_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .map_err(|e| format!("OpenAI Costs API failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("OpenAI Costs API returned {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("cannot parse OpenAI costs response: {e}"))?;

    parse_costs_response(&body)
}

fn parse_completions_response(body: &serde_json::Value) -> Result<Vec<UsageBucket>, String> {
    let mut buckets = Vec::new();

    let data = body.get("data").and_then(|d| d.as_array());
    let data = match data {
        Some(d) => d,
        None => return Ok(buckets),
    };

    for bucket in data {
        let start_time = bucket
            .get("start_time")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let date = timestamp_to_date(start_time);

        let results = match bucket
            .get("result")
            .and_then(|r| r.as_array())
            .or_else(|| bucket.get("results").and_then(|r| r.as_array()))
        {
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
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = result
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cached = result
                .get("input_cached_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let requests = result
                .get("num_model_requests")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            if input == 0 && output == 0 {
                continue;
            }

            buckets.push(UsageBucket {
                source: "openai".to_string(),
                date: date.clone(),
                model,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cached,
                cache_creation_tokens: 0,
                cost_usd: 0.0,
                requests,
            });
        }
    }

    Ok(buckets)
}

fn parse_costs_response(body: &serde_json::Value) -> Result<Vec<UsageBucket>, String> {
    let mut buckets = Vec::new();

    let data = body.get("data").and_then(|d| d.as_array());
    let data = match data {
        Some(d) => d,
        None => return Ok(buckets),
    };

    for bucket in data {
        let start_time = bucket
            .get("start_time")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let date = timestamp_to_date(start_time);

        let results = match bucket
            .get("result")
            .and_then(|r| r.as_array())
            .or_else(|| bucket.get("results").and_then(|r| r.as_array()))
        {
            Some(r) => r,
            None => continue,
        };

        for result in results {
            let cost_usd = result
                .get("amount")
                .and_then(|a| a.get("value"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            let line_item = result
                .get("line_item")
                .and_then(|l| l.as_str())
                .map(String::from);

            if cost_usd == 0.0 {
                continue;
            }

            buckets.push(UsageBucket {
                source: "openai".to_string(),
                date: date.clone(),
                model: line_item,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd,
                requests: 0,
            });
        }
    }

    Ok(buckets)
}

fn timestamp_to_date(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_completions() {
        let body: serde_json::Value = serde_json::json!({
            "data": [
                {
                    "start_time": 1709596800,
                    "result": [
                        {
                            "model": "gpt-5.2",
                            "input_tokens": 10000,
                            "output_tokens": 3000,
                            "input_cached_tokens": 2000,
                            "num_model_requests": 5
                        }
                    ]
                }
            ]
        });

        let buckets = parse_completions_response(&body).unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].model.as_deref(), Some("gpt-5.2"));
        assert_eq!(buckets[0].input_tokens, 10000);
        assert_eq!(buckets[0].output_tokens, 3000);
        assert_eq!(buckets[0].cache_read_tokens, 2000);
        assert_eq!(buckets[0].requests, 5);
    }

    #[test]
    fn test_parse_costs() {
        let body: serde_json::Value = serde_json::json!({
            "data": [
                {
                    "start_time": 1709596800,
                    "result": [
                        {
                            "line_item": "completions",
                            "amount": { "currency": "usd", "value": 2.50 }
                        }
                    ]
                }
            ]
        });

        let buckets = parse_costs_response(&body).unwrap();
        assert_eq!(buckets.len(), 1);
        assert!((buckets[0].cost_usd - 2.50).abs() < 0.01);
    }

    #[test]
    fn test_timestamp_to_date() {
        let d = timestamp_to_date(1709596800);
        assert!(!d.is_empty());
        assert!(d.starts_with("2024-03"));
    }
}
