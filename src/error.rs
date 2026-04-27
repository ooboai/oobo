#[derive(Debug, thiserror::Error)]
pub enum OoboError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("config: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, OoboError>;

impl From<String> for OoboError {
    fn from(s: String) -> Self {
        OoboError::Other(s)
    }
}

impl From<&str> for OoboError {
    fn from(s: &str) -> Self {
        OoboError::Other(s.to_string())
    }
}
