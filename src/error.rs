use thiserror::Error;

// ---------------------------------------------------------------------------
// OoboError  --  general-purpose library error (pre-existing)
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
// CliError  --  command-layer error with exit-code mapping
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

    /// Content destined for shared storage was blocked because it contains a
    /// detected secret. This is a deterministic, non-retryable refusal.
    #[error("refusing to publish: secret detected in content destined for shared storage")]
    SecretBlocked,

    #[error("{0}")]
    Config(String),

    #[error("{0}")]
    Remote(String),

    /// Sentinel: signals main() to run the MCP server outside the tokio runtime.
    #[error("mcp")]
    McpRun {
        api_key: Option<String>,
        api_url: String,
    },
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::User(_) => 2,
            Self::NotARepo
            | Self::Io { .. }
            | Self::Git(_)
            | Self::SecretBlocked
            | Self::Config(_)
            | Self::Remote(_) => 1,
            Self::McpRun { .. } => 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_blocked_is_matchable_by_pattern() {
        let err = CliError::SecretBlocked;
        assert!(
            matches!(err, CliError::SecretBlocked),
            "SecretBlocked must be matchable via matches! macro"
        );
    }

    #[test]
    fn secret_blocked_is_distinct_from_git_error() {
        let git_err = CliError::Git("secret detected".into());
        assert!(
            !matches!(git_err, CliError::SecretBlocked),
            "Git error with 'secret detected' text must NOT match SecretBlocked variant"
        );
    }

    #[test]
    fn secret_blocked_exit_code_is_one() {
        assert_eq!(CliError::SecretBlocked.exit_code(), 1);
    }

    #[test]
    fn secret_blocked_display_message() {
        let err = CliError::SecretBlocked;
        let msg = err.to_string();
        assert!(
            msg.contains("secret"),
            "display message should mention secret: {msg}"
        );
    }
}
