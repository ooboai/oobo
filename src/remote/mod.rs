pub mod payload;

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

fn endpoint_with_base(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

async fn authenticated_post(
    base_url: &str,
    path: &str,
    api_key: &str,
    body: &impl serde::Serialize,
    timeout: std::time::Duration,
) -> RemoteResult<reqwest::Response> {
    let url = endpoint_with_base(base_url, path);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(timeout)
        .build()
        .map_err(|e| RemoteError::Client(e.to_string()))?;

    client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", format!("oobo/{}", env!("CARGO_PKG_VERSION")))
        .json(body)
        .send()
        .await
        .map_err(|e| RemoteError::Request(e.to_string()))
}

fn map_common_errors(status: reqwest::StatusCode) -> Option<RemoteError> {
    if status.as_u16() == 401 {
        return Some(RemoteError::Auth(
            "invalid or missing API key  --  run: oobo settings set key <KEY>".to_string(),
        ));
    }
    None
}

async fn map_server_error(status: reqwest::StatusCode, resp: reqwest::Response) -> RemoteError {
    if status.is_server_error() {
        let body = resp.text().await.unwrap_or_default();
        return RemoteError::Server(if body.is_empty() {
            format!("HTTP {status}")
        } else {
            body
        });
    }
    RemoteError::Http(format!("HTTP {status}"))
}

pub async fn search_anchors_with_timeout(
    request: &payload::SearchRequest,
    api_key: &str,
    base_url: &str,
    timeout: std::time::Duration,
) -> RemoteResult<payload::SearchResponse> {
    let resp = authenticated_post(base_url, "anchors/search", api_key, request, timeout).await?;
    let status = resp.status();

    if status.is_success() {
        return resp
            .json::<payload::SearchResponse>()
            .await
            .map_err(|e| RemoteError::Parse(e.to_string()));
    }

    if let Some(e) = map_common_errors(status) {
        return Err(e);
    }

    if status.as_u16() == 422 {
        let detail = resp.text().await.unwrap_or_default();
        return Err(RemoteError::Rejected(if detail.is_empty() {
            "search request rejected (422)".to_string()
        } else {
            format!("search request rejected (422): {detail}")
        }));
    }

    Err(map_server_error(status, resp).await)
}

pub async fn post_delta(
    request: &payload::DeltaRequest,
    api_key: &str,
    base_url: &str,
    timeout: std::time::Duration,
) -> RemoteResult<payload::DeltaResponse> {
    let resp = authenticated_post(base_url, "anchors/delta", api_key, request, timeout).await?;
    let status = resp.status();

    if status.is_success() {
        return resp
            .json::<payload::DeltaResponse>()
            .await
            .map_err(|e| RemoteError::Parse(e.to_string()));
    }

    if let Some(e) = map_common_errors(status) {
        return Err(e);
    }

    if status.as_u16() == 404 {
        let body: payload::DeltaErrorResponse =
            resp.json().await.unwrap_or(payload::DeltaErrorResponse {
                error: Some("anchor_not_found".into()),
                message: Some("anchor not found".into()),
            });
        return Err(RemoteError::Rejected(
            body.message.unwrap_or_else(|| "anchor not found".into()),
        ));
    }

    Err(map_server_error(status, resp).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_joins_without_double_slashes() {
        assert_eq!(
            endpoint_with_base("https://example.com/", "/anchors/search"),
            "https://example.com/anchors/search"
        );
        assert_eq!(
            endpoint_with_base("https://example.com", "anchors/search"),
            "https://example.com/anchors/search"
        );
    }
}
