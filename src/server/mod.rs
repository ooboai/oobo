pub mod payload;

use crate::config::Config;

/// Fire-and-forget: spawn a background thread to POST the event.
/// Never blocks the caller. Errors are silently ignored.
pub fn send_event(cfg: &Config, payload: &payload::EventPayload) {
    let url = format!("{}/api/v1/events", cfg.server.url.trim_end_matches('/'));
    let api_key = cfg.server.api_key.clone();
    let body = match serde_json::to_string(payload) {
        Ok(b) => b,
        Err(_) => return,
    };

    std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                format!("oobo-git/{}", env!("CARGO_PKG_VERSION")),
            )
            .body(body)
            .send();
    });
}

/// Synchronous check: can we reach the dashboard?
pub fn check_connection(cfg: &Config) -> Result<String, String> {
    let url = format!("{}/api/v1/health", cfg.server.url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("client error: {e}"))?;

    let resp = client
        .get(&url)
        .header(
            "User-Agent",
            format!("oobo-git/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .map_err(|e| format!("connection failed: {e}"))?;

    let status = resp.status();
    if status.is_success() {
        Ok(format!("connected (HTTP {status})"))
    } else {
        Err(format!("server returned HTTP {status}"))
    }
}
