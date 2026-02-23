pub mod payload;

use crate::config::Config;

/// Fire-and-forget: spawn a short-lived async task to POST the event.
/// Never blocks the caller. Errors are silently logged.
pub fn send_event(cfg: &Config, payload: &payload::EventPayload) {
    let url = format!("{}/api/v1/events", cfg.server.url.trim_end_matches('/'));
    let api_key = cfg.server.api_key.clone();
    let body = match serde_json::to_string(payload) {
        Ok(b) => b,
        Err(_) => return,
    };

    // Spawn a background thread so we never block git
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return,
        };

        rt.block_on(async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build();

            let client = match client {
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
                .send()
                .await;
        });
    });
}

/// Synchronous check: can we reach the dashboard?
pub fn check_connection(cfg: &Config) -> Result<String, String> {
    let url = format!("{}/api/v1/health", cfg.server.url.trim_end_matches('/'));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime error: {e}"))?;

    rt.block_on(async {
        let client = reqwest::Client::builder()
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
            .await
            .map_err(|e| format!("connection failed: {e}"))?;

        let status = resp.status();
        if status.is_success() {
            Ok(format!("connected (HTTP {status})"))
        } else {
            Err(format!("server returned HTTP {status}"))
        }
    })
}
