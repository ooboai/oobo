pub mod payload;

use crate::config::Config;

/// Spawn a background thread to POST the anchor to the ingestion API.
/// Returns the JoinHandle so the caller can wait if needed (e.g. from a
/// post-commit hook where the process would otherwise exit immediately).
pub fn send_event(
    cfg: &Config,
    payload: &payload::EventPayload,
    api_key_override: Option<&str>,
) -> std::thread::JoinHandle<()> {
    let url = format!("{}/anchors/ingest", cfg.server.url.trim_end_matches('/'));
    let api_key = api_key_override.unwrap_or(&cfg.server.api_key).to_string();
    let body = match serde_json::to_string(payload) {
        Ok(b) => b,
        Err(_) => return std::thread::spawn(|| {}),
    };

    std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };

        let resp = match client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", format!("oobo/{}", env!("CARGO_PKG_VERSION")))
            .body(body)
            .send()
        {
            Ok(r) => r,
            Err(_) => return,
        };

        let status = resp.status();

        if status.is_success() {
            return;
        }

        // Duplicate anchor → (git_remote, commit_hash) unique constraint.
        // Treat 409 as success.
        if status.as_u16() == 409 {
            return;
        }

        if status.as_u16() == 500 {
            let body = resp.text().unwrap_or_default();
            if body.contains("duplicate") || body.contains("unique") || body.contains("UNIQUE") {
                return;
            }
            eprintln!("oobo: warning: server error during sync: {body}");
            return;
        }

        if status.as_u16() == 401 {
            if let Ok(err) = resp.json::<payload::IngestError>() {
                let detail = err
                    .detail
                    .unwrap_or_else(|| "invalid or missing API key".into());
                eprintln!("oobo: sync auth error: {detail}");
                eprintln!("      run `oobo sync on` to reconfigure.");
            }
            return;
        }

        if status.as_u16() == 422 {
            let detail = resp.text().unwrap_or_default();
            if detail.is_empty() {
                eprintln!("oobo: warning: sync payload rejected (422). Run `oobo --version` to check for updates.");
            } else {
                eprintln!("oobo: warning: sync payload rejected (422): {detail}");
            }
        }
    })
}

/// Synchronous anchor ingestion — returns the parsed response.
/// Used by explicit `oobo sync` commands and tests.
#[allow(dead_code)]
pub fn ingest_anchor(
    cfg: &Config,
    payload: &payload::EventPayload,
) -> Result<payload::IngestResponse, String> {
    let url = format!("{}/anchors/ingest", cfg.server.url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("client error: {e}"))?;

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cfg.server.api_key))
        .header("Content-Type", "application/json")
        .header("User-Agent", format!("oobo/{}", env!("CARGO_PKG_VERSION")))
        .json(payload)
        .send()
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();

    if status.is_success() {
        return resp
            .json::<payload::IngestResponse>()
            .map_err(|e| format!("cannot parse response: {e}"));
    }

    let body = resp.text().unwrap_or_default();

    // Duplicate → treat as success.
    if body.contains("duplicate") || body.contains("unique") || body.contains("UNIQUE") {
        return Ok(payload::IngestResponse {
            success: true,
            message: "Anchor already ingested (duplicate)".into(),
            data: None,
        });
    }

    if let Ok(err) = serde_json::from_str::<payload::IngestError>(&body) {
        let msg = err
            .detail
            .or(err.message)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(msg);
    }

    Err(format!("HTTP {status}: {body}"))
}

/// Synchronous check: can we reach the API?
pub fn check_connection(cfg: &Config) -> Result<String, String> {
    let url = format!("{}/anchors/health", cfg.server.url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("client error: {e}"))?;

    let resp = client
        .get(&url)
        .header("User-Agent", format!("oobo/{}", env!("CARGO_PKG_VERSION")))
        .send()
        .map_err(|e| format!("connection failed: {e}"))?;

    let status = resp.status();
    if status.is_success() {
        Ok(format!("connected (HTTP {status})"))
    } else {
        Err(format!("server returned HTTP {status}"))
    }
}
