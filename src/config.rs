use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const DEFAULT_SERVER_URL: &str = "https://api.oobo.ai";

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
    pub gemini: ToolConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub transparency: TransparencyConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored_repos: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_server_url")]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub sync: bool,
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
    /// Optional API key for pulling usage data from the tool's cloud API.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub send_diffs: bool,
    #[serde(default)]
    pub send_transcripts: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    #[serde(default = "default_true")]
    pub auto_scan: bool,
    #[serde(default = "default_scan_interval")]
    pub interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfig {
    #[serde(default = "default_true")]
    pub check_on_startup: bool,
    #[serde(default = "default_update_interval")]
    pub check_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransparencyConfig {
    #[serde(default = "default_transparency_mode")]
    pub mode: String,
}

impl Default for TransparencyConfig {
    fn default() -> Self {
        Self {
            mode: default_transparency_mode(),
        }
    }
}

fn default_transparency_mode() -> String {
    "off".to_string()
}

fn default_scan_interval() -> u64 {
    3600
}

fn default_update_interval() -> u64 {
    86400
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
            sync: false,
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
        Self {
            enabled: true,
            api_key: String::new(),
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            send_diffs: false,
            send_transcripts: false,
        }
    }
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            auto_scan: true,
            interval_secs: default_scan_interval(),
        }
    }
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            check_interval_secs: default_update_interval(),
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        crate::paths::oobo_home().join("config.toml")
    }

    pub fn log_dir() -> PathBuf {
        crate::paths::oobo_home().join("logs")
    }

    /// Load config from disk, or return defaults if it doesn't exist.
    /// `OOBO_SECRET_KEY` env var overrides the persisted `api_key` when set.
    pub fn load_or_default() -> Self {
        let path = Self::config_path();
        let mut cfg = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        eprintln!("oobo: warning: invalid config at {}: {e}", path.display());
                        Self::default()
                    }
                },
                Err(e) => {
                    eprintln!("oobo: warning: cannot read {}: {e}", path.display());
                    Self::default()
                }
            }
        } else {
            Self::default()
        };

        if let Ok(key) = std::env::var("OOBO_SECRET_KEY") {
            if !key.is_empty() {
                cfg.server.api_key = key;
            }
        }

        cfg
    }

    /// Persist config to disk. Sets file permissions to 0600 on Unix when API keys are present.
    pub fn save(&self) -> Result<(), String> {
        let dir = crate::paths::oobo_home();
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

        let content =
            toml::to_string_pretty(self).map_err(|e| format!("cannot serialize config: {e}"))?;

        let path = Self::config_path();
        fs::write(&path, &content).map_err(|e| format!("cannot write {}: {e}", path.display()))?;

        #[cfg(unix)]
        if self.has_any_key() {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
                eprintln!("oobo: warning: could not set config permissions: {e}");
            }
        }

        Ok(())
    }

    fn has_any_key(&self) -> bool {
        !self.server.api_key.is_empty()
            || !self.claude.api_key.is_empty()
            || !self.cursor.api_key.is_empty()
            || !self.copilot.api_key.is_empty()
            || !self.windsurf.api_key.is_empty()
            || !self.codex.api_key.is_empty()
            || !self.gemini.api_key.is_empty()
            || !self.opencode.api_key.is_empty()
            || !self.aider.api_key.is_empty()
            || !self.trae.api_key.is_empty()
    }

    /// True when sync is enabled and an API key is available.
    #[allow(dead_code)]
    pub fn should_sync(&self) -> bool {
        self.server.sync && !self.server.api_key.is_empty()
    }

    /// True if the server is configured with a non-default API key.
    #[cfg(test)]
    pub fn is_configured(&self) -> bool {
        !self.server.api_key.is_empty()
    }

    /// Resolve the configured transparency mode.
    /// Transparency only controls whether redacted transcripts are included on the
    /// orphan branch. Anchor metadata is always written regardless of this setting.
    pub fn transparency_mode(&self) -> crate::core::anchor::TransparencyMode {
        match self.transparency.mode.as_str() {
            "on" | "full" | "full_transparency" => crate::core::anchor::TransparencyMode::On,
            _ => crate::core::anchor::TransparencyMode::Off,
        }
    }

    /// Resolve the real git binary path.
    pub fn git_path(&self) -> &str {
        if self.git.real_git_path.is_empty() {
            "git"
        } else {
            &self.git.real_git_path
        }
    }

    /// Set a tool's enabled state by config key.
    pub fn set_tool_enabled(&mut self, key: &str, enabled: bool) {
        match key {
            "cursor" => self.cursor.enabled = enabled,
            "claude" => self.claude.enabled = enabled,
            "windsurf" => self.windsurf.enabled = enabled,
            "aider" => self.aider.enabled = enabled,
            "zed" => self.zed.enabled = enabled,
            "copilot" => self.copilot.enabled = enabled,
            "trae" => self.trae.enabled = enabled,
            "codex" => self.codex.enabled = enabled,
            "opencode" => self.opencode.enabled = enabled,
            "gemini" => self.gemini.enabled = enabled,
            _ => {}
        }
    }

    /// Check if a repo path is in the ignored list.
    pub fn is_ignored(&self, project_root: &str) -> bool {
        let canonical = std::fs::canonicalize(project_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| project_root.to_string());
        self.ignored_repos.iter().any(|p| {
            let c = std::fs::canonicalize(p)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.clone());
            c == canonical
        })
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
        assert_eq!(cfg.server.url, "https://api.oobo.ai");
        assert!(cfg.server.api_key.is_empty());
        assert!(!cfg.server.sync);
        assert!(!cfg.should_sync());
        assert!(!cfg.git.alias_enabled);
        assert!(cfg.cursor.enabled);
        assert!(cfg.claude.enabled);
        assert!(cfg.windsurf.enabled);
        assert!(cfg.aider.enabled);
        assert!(cfg.zed.enabled);
        assert!(cfg.copilot.enabled);
        assert!(cfg.trae.enabled);
        assert!(cfg.opencode.enabled);
        assert!(cfg.gemini.enabled);
        assert!(cfg.telemetry.enabled);
        assert!(!cfg.telemetry.send_diffs);
    }

    #[test]
    fn test_roundtrip_toml() {
        let cfg = Config {
            server: ServerConfig {
                url: "https://my.server.com".into(),
                api_key: "test_key_123".into(),
                sync: true,
            },
            git: GitConfig {
                real_git_path: "/usr/bin/git".into(),
                alias_enabled: true,
            },
            cursor: ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            claude: ToolConfig {
                enabled: true,
                api_key: String::new(),
            },
            windsurf: ToolConfig {
                enabled: true,
                api_key: String::new(),
            },
            aider: ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            zed: ToolConfig {
                enabled: true,
                api_key: String::new(),
            },
            copilot: ToolConfig {
                enabled: true,
                api_key: String::new(),
            },
            trae: ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            codex: ToolConfig {
                enabled: true,
                api_key: String::new(),
            },
            opencode: ToolConfig {
                enabled: true,
                api_key: String::new(),
            },
            gemini: ToolConfig {
                enabled: true,
                api_key: String::new(),
            },
            telemetry: TelemetryConfig {
                enabled: true,
                send_diffs: true,
                send_transcripts: false,
            },
            scan: ScanConfig::default(),
            update: UpdateConfig::default(),
            transparency: TransparencyConfig::default(),
            ignored_repos: Vec::new(),
        };
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.server.url, "https://my.server.com");
        assert_eq!(deserialized.server.api_key, "test_key_123");
        assert!(deserialized.server.sync);
        assert!(deserialized.should_sync());
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
        assert!(!path.is_empty());
    }

    #[test]
    fn test_is_ignored_empty() {
        let cfg = Config::default();
        assert!(!cfg.is_ignored("/some/path"));
    }

    #[test]
    fn test_is_ignored_with_real_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let mut cfg = Config::default();
        cfg.ignored_repos.push(path.clone());
        assert!(cfg.is_ignored(&path));
        assert!(!cfg.is_ignored("/nonexistent-oobo-test"));
    }

    #[test]
    fn test_ignore_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let mut cfg = Config::default();
        cfg.ignored_repos.push(path.clone());
        let before = cfg.ignored_repos.len();
        let canonical = std::fs::canonicalize(&path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(path.clone());
        if !cfg.is_ignored(&canonical) {
            cfg.ignored_repos.push(canonical);
        }
        assert_eq!(cfg.ignored_repos.len(), before);
    }

    #[test]
    fn test_unignore_removes_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let canonical = std::fs::canonicalize(&path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(path.clone());
        let mut cfg = Config::default();
        cfg.ignored_repos.push(canonical.clone());
        assert!(cfg.is_ignored(&path));
        cfg.ignored_repos.retain(|p| {
            let c = std::fs::canonicalize(p)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.clone());
            c != canonical
        });
        assert!(!cfg.is_ignored(&path));
    }

    #[test]
    fn test_has_any_key_includes_opencode() {
        let mut cfg = Config::default();
        assert!(!cfg.has_any_key());
        cfg.opencode.api_key = "sk-test".to_string();
        assert!(cfg.has_any_key());
    }

    #[test]
    fn test_has_any_key_includes_aider_and_trae() {
        let mut cfg = Config::default();
        assert!(!cfg.has_any_key());
        cfg.aider.api_key = "sk-aider".to_string();
        assert!(cfg.has_any_key());

        let mut cfg = Config::default();
        cfg.trae.api_key = "sk-trae".to_string();
        assert!(cfg.has_any_key());
    }
}
