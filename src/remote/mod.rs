pub mod payload;

use crate::config::Config;

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("client: {0}")]
    Client(String),
    #[error("request: {0}")]
    Request(String),
    #[error("cannot parse response: {0}")]
    Parse(String),
    #[error("auth: {0}")]
    Auth(String),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("server: {0}")]
    Server(String),
    #[error("http: {0}")]
    Http(String),
}

pub type RemoteResult<T> = Result<T, RemoteError>;

impl From<RemoteError> for String {
    fn from(error: RemoteError) -> Self {
        error.to_string()
    }
}

pub fn effective_server_url(cfg: &Config) -> String {
    cfg.server.url.clone()
}

fn endpoint(cfg: &Config, path: &str) -> String {
    format!(
        "{}/{}",
        effective_server_url(cfg).trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub async fn search_anchors_with_timeout(
    cfg: &Config,
    request: &payload::SearchRequest,
    api_key_override: Option<&str>,
    timeout: std::time::Duration,
) -> RemoteResult<payload::SearchResponse> {
    let url = endpoint(cfg, "anchors/search");
    let api_key = api_key_override.unwrap_or(&cfg.server.api_key);

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(timeout)
        .build()
        .map_err(|e| RemoteError::Client(e.to_string()))?;

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", format!("oobo/{}", env!("CARGO_PKG_VERSION")))
        .json(request)
        .send()
        .await
        .map_err(|e| RemoteError::Request(e.to_string()))?;

    let status = resp.status();
    if status.is_success() {
        return resp
            .json::<payload::SearchResponse>()
            .await
            .map_err(|e| RemoteError::Parse(e.to_string()));
    }

    if status.as_u16() == 401 {
        if let Ok(err) = resp.json::<payload::IngestError>().await {
            let detail = err
                .detail
                .or(err.message)
                .unwrap_or_else(|| "invalid or missing API key".into());
            return Err(RemoteError::Auth(detail));
        }
        return Err(RemoteError::Auth("invalid or missing API key".to_string()));
    }

    if status.as_u16() == 422 {
        let detail = resp.text().await.unwrap_or_default();
        if detail.is_empty() {
            return Err(RemoteError::Rejected(
                "search request rejected (422)".to_string(),
            ));
        }
        return Err(RemoteError::Rejected(format!(
            "search request rejected (422): {detail}"
        )));
    }

    if status.is_server_error() {
        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            return Err(RemoteError::Server(format!("HTTP {status}")));
        }
        return Err(RemoteError::Server(body));
    }

    Err(RemoteError::Http(format!("HTTP {status}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_server_url_falls_back_to_global_config_outside_repo() {
        let cfg = Config {
            server: crate::config::ServerConfig {
                url: "https://global.example.com".to_string(),
                ..Default::default()
            },
            git: crate::config::GitConfig {
                real_git_path: "/no/such/git".to_string(),
                ..Default::default()
            },
            ..Config::default()
        };

        assert_eq!(effective_server_url(&cfg), "https://global.example.com");
    }

    #[test]
    fn endpoint_joins_without_double_slashes() {
        let cfg = Config {
            server: crate::config::ServerConfig {
                url: "https://global.example.com/".to_string(),
                ..Default::default()
            },
            git: crate::config::GitConfig {
                real_git_path: "/no/such/git".to_string(),
                ..Default::default()
            },
            ..Config::default()
        };

        assert_eq!(
            endpoint(&cfg, "/anchors/search"),
            "https://global.example.com/anchors/search"
        );
    }
}
