use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const DEFAULT_SERVER_URL: &str = "https://dashboard.oobo.ai";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub git: GitConfig,
    #[serde(default)]
    pub cursor: ToolConfig,
    #[serde(default)]
    pub claude: ToolConfig,
    #[serde(default)]
    pub windsurf: ToolConfig,
    #[serde(default)]
    pub aider: ToolConfig,
    #[serde(default)]
    pub continue_dev: ToolConfig,
    #[serde(default)]
    pub zed: ToolConfig,
    #[serde(default)]
    pub copilot: ToolConfig,
    #[serde(default)]
    pub trae: ToolConfig,
    #[serde(default)]
    pub codex: ToolConfig,
    #[serde(default)]
    pub opencode: ToolConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_server_url")]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default)]
    pub real_git_path: String,
    #[serde(default)]
    pub alias_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[allow(dead_code)]
pub type CursorConfig = ToolConfig;
#[allow(dead_code)]
pub type ClaudeConfig = ToolConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub send_diffs: bool,
}

fn default_server_url() -> String {
    DEFAULT_SERVER_URL.to_string()
}

fn default_true() -> bool {
    true
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            url: default_server_url(),
            api_key: String::new(),
        }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            real_git_path: find_real_git().unwrap_or_default(),
            alias_enabled: false,
        }
    }
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            send_diffs: false,
        }
    }
}

impl Config {
    /// Directory where oobo stores its configuration and logs.
    pub fn oobo_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".oobo")
    }

    pub fn config_path() -> PathBuf {
        Self::oobo_dir().join("config.toml")
    }

    pub fn log_dir() -> PathBuf {
        Self::oobo_dir().join("logs")
    }

    /// Load config from disk, or return defaults if it doesn't exist.
    pub fn load_or_default() -> Self {
        let path = Self::config_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(cfg) => return cfg,
                    Err(e) => {
                        eprintln!("oobo: warning: invalid config at {}: {e}", path.display());
                    }
                },
                Err(e) => {
                    eprintln!("oobo: warning: cannot read {}: {e}", path.display());
                }
            }
        }
        Self::default()
    }

    /// Persist config to disk.
    pub fn save(&self) -> Result<(), String> {
        let dir = Self::oobo_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

        let content =
            toml::to_string_pretty(self).map_err(|e| format!("cannot serialize config: {e}"))?;

        let path = Self::config_path();
        fs::write(&path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(())
    }

    /// True if the server is configured with a non-default API key.
    #[allow(dead_code)]
    pub fn is_configured(&self) -> bool {
        !self.server.api_key.is_empty()
    }

    /// Resolve the real git binary path.
    pub fn git_path(&self) -> &str {
        if self.git.real_git_path.is_empty() {
            "git"
        } else {
            &self.git.real_git_path
        }
    }
}

/// Find the real git binary, skipping any `oobo` alias.
pub fn find_real_git() -> Option<String> {
    // `which -a git` lists all git binaries in PATH
    let output = std::process::Command::new("which")
        .arg("-a")
        .arg("git")
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Skip if it resolves to oobo
        if let Ok(resolved) = fs::canonicalize(line) {
            let name = resolved.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "oobo" {
                continue;
            }
        }
        return Some(line.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.server.url, DEFAULT_SERVER_URL);
        assert!(cfg.server.api_key.is_empty());
        assert!(!cfg.git.alias_enabled);
        assert!(cfg.cursor.enabled);
        assert!(cfg.claude.enabled);
        assert!(cfg.windsurf.enabled);
        assert!(cfg.aider.enabled);
        assert!(cfg.continue_dev.enabled);
        assert!(cfg.zed.enabled);
        assert!(cfg.copilot.enabled);
        assert!(cfg.trae.enabled);
        assert!(cfg.opencode.enabled);
        assert!(cfg.telemetry.enabled);
        assert!(!cfg.telemetry.send_diffs);
    }

    #[test]
    fn test_roundtrip_toml() {
        let cfg = Config {
            server: ServerConfig {
                url: "https://my.server.com".into(),
                api_key: "sk_test_123".into(),
            },
            git: GitConfig {
                real_git_path: "/usr/bin/git".into(),
                alias_enabled: true,
            },
            cursor: CursorConfig { enabled: false },
            claude: ClaudeConfig { enabled: true },
            windsurf: ToolConfig { enabled: true },
            aider: ToolConfig { enabled: false },
            continue_dev: ToolConfig { enabled: true },
            zed: ToolConfig { enabled: true },
            copilot: ToolConfig { enabled: true },
            trae: ToolConfig { enabled: false },
            codex: ToolConfig { enabled: true },
            opencode: ToolConfig { enabled: true },
            telemetry: TelemetryConfig {
                enabled: true,
                send_diffs: true,
            },
        };
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.server.url, "https://my.server.com");
        assert_eq!(deserialized.server.api_key, "sk_test_123");
        assert!(deserialized.git.alias_enabled);
        assert!(!deserialized.cursor.enabled);
        assert!(deserialized.claude.enabled);
        assert!(!deserialized.aider.enabled);
        assert!(!deserialized.trae.enabled);
        assert!(deserialized.telemetry.send_diffs);
    }

    #[test]
    fn test_config_not_configured_by_default() {
        let cfg = Config::default();
        assert!(!cfg.is_configured());
    }

    #[test]
    fn test_git_path_fallback() {
        let cfg = Config::default();
        let path = cfg.git_path();
        // Should return something (either found git or fallback "git")
        assert!(!path.is_empty());
    }
}
