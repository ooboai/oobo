use thiserror::Error;

// ---------------------------------------------------------------------------
// OoboError — general-purpose library error (pre-existing)
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
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

// ---------------------------------------------------------------------------
// CliError — command-layer error with exit-code mapping
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    User(String),

    #[error("not inside a git repository.")]
    NotARepo,

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Git(String),

    #[error("{0}")]
    Config(String),

    #[error("{0}")]
    Remote(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::User(_) => 2,
            Self::NotARepo | Self::Io { .. } | Self::Git(_) | Self::Config(_) | Self::Remote(_) => 1,
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            context: "i/o error".into(),
            source: e,
        }
    }
}

impl From<String> for CliError {
    fn from(s: String) -> Self {
        Self::User(s)
    }
}

impl From<crate::remote::RemoteError> for CliError {
    fn from(e: crate::remote::RemoteError) -> Self {
        Self::Remote(e.to_string())
    }
}

/// Command result type. `Ok(i32)` is the exit code (0 = success).
pub type CmdResult = std::result::Result<i32, CliError>;
