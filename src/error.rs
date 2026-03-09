use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum OoboError {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("config: {0}")]
    Config(String),

    #[error("git: {0}")]
    Git(String),

    #[error("tool ({tool}): {message}")]
    Tool { tool: String, message: String },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("path: {0}")]
    Path(PathBuf),

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
