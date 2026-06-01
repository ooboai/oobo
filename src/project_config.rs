use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const PROJECT_CONFIG_DIR: &str = ".oobo";
const PROJECT_CONFIG_FILE: &str = "config";
const PROJECT_SECRETS_FILE: &str = "secrets";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub project: ProjectSection,
    #[serde(default, skip_serializing_if = "ProjectServerSection::is_empty")]
    pub server: ProjectServerSection,
    #[serde(default)]
    pub anchors: AnchorsSection,
    #[serde(default)]
    pub privacy: PrivacySection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSection {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

impl Default for ProjectSection {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectServerSection {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

impl ProjectServerSection {
    fn is_empty(&self) -> bool {
        self.api_key.is_empty() && self.url.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnchorsSection {
    /// Git remote target for the anchor metadata branch.
    /// Defaults to [`crate::config::DEFAULT_ANCHOR_REMOTE`].
    /// May be a configured Git remote name (`oobo`) or a full Git URL.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacySection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparency: Option<String>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub redact: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            project: ProjectSection::default(),
            server: ProjectServerSection::default(),
            anchors: AnchorsSection::default(),
            privacy: PrivacySection::default(),
        }
    }
}

impl Default for PrivacySection {
    fn default() -> Self {
        Self {
            transparency: None,
            redact: true,
        }
    }
}

impl ProjectConfig {
    pub fn for_project(project_id: &str) -> Self {
        Self {
            project: ProjectSection {
                id: project_id.to_string(),
                enabled: true,
            },
            ..Self::default()
        }
    }

    pub fn load(project_root: &str) -> Result<Option<Self>, String> {
        let path = path_for(project_root);
        if !path.exists() {
            return Ok(None);
        }
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let text = strip_utf8_bom(&bytes);
        let mut cfg: Self = toml::from_str(text)
            .map_err(|e| format!("invalid project config at {}: {e}", path.display()))?;

        // Merge secrets from the gitignored secrets file
        let secrets_path = secrets_path_for(project_root);
        if secrets_path.exists() {
            if let Ok(bytes) = std::fs::read(&secrets_path) {
                let secrets_text = strip_utf8_bom(&bytes);
                if let Ok(secrets) = toml::from_str::<ProjectSecrets>(secrets_text) {
                    if !secrets.api_key.is_empty() {
                        cfg.server.api_key = secrets.api_key;
                    }
                }
            }
        }

        Ok(Some(cfg))
    }

    pub fn save(&self, project_root: &str) -> Result<(), String> {
        let path = path_for(project_root);
        let dir = path
            .parent()
            .ok_or_else(|| format!("invalid project config path {}", path.display()))?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

        ensure_gitignore(dir);

        // Write secrets (api_key) to a separate gitignored file
        let secrets_path = secrets_path_for(project_root);
        if !self.server.api_key.is_empty() {
            let secrets = ProjectSecrets {
                api_key: self.server.api_key.clone(),
            };
            let secrets_content = toml::to_string_pretty(&secrets)
                .map_err(|e| format!("cannot serialize secrets: {e}"))?;
            let tmp_secrets = secrets_path.with_extension("tmp");
            std::fs::write(&tmp_secrets, &secrets_content)
                .map_err(|e| format!("cannot write {}: {e}", tmp_secrets.display()))?;
            std::fs::rename(&tmp_secrets, &secrets_path)
                .map_err(|e| format!("cannot rename secrets: {e}"))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&secrets_path, std::fs::Permissions::from_mode(0o600));
            }
        } else if secrets_path.exists() {
            // Key was unset — remove the secrets file
            let _ = std::fs::remove_file(&secrets_path);
        }

        // Write config without the api_key
        let mut saveable = self.clone();
        saveable.server.api_key = String::new();

        let content = toml::to_string_pretty(&saveable)
            .map_err(|e| format!("cannot serialize project config: {e}"))?;
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, content)
            .map_err(|e| format!("cannot write {}: {e}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &path)
            .map_err(|e| format!("cannot rename project config: {e}"))?;
        Ok(())
    }
}

pub fn path_for(project_root: &str) -> PathBuf {
    Path::new(project_root)
        .join(PROJECT_CONFIG_DIR)
        .join(PROJECT_CONFIG_FILE)
}

pub fn secrets_path_for(project_root: &str) -> PathBuf {
    Path::new(project_root)
        .join(PROJECT_CONFIG_DIR)
        .join(PROJECT_SECRETS_FILE)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectSecrets {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    api_key: String,
}

pub fn exists(project_root: &str) -> bool {
    path_for(project_root).exists()
}

pub fn is_enabled(project_root: &str) -> bool {
    ProjectConfig::load(project_root)
        .ok()
        .flatten()
        .is_some_and(|cfg| cfg.project.enabled)
}

pub fn set_enabled(project_root: &str, project_id: &str, enabled: bool) -> Result<bool, String> {
    let mut cfg = ProjectConfig::load(project_root)?
        .unwrap_or_else(|| ProjectConfig::for_project(project_id));
    if cfg.project.id.is_empty() {
        cfg.project.id = project_id.to_string();
    }
    let changed = cfg.project.enabled != enabled || !exists(project_root);
    cfg.project.enabled = enabled;
    cfg.save(project_root)?;
    Ok(changed)
}

pub fn transparency_mode(project_root: &str) -> Option<crate::core::anchor::TransparencyMode> {
    let cfg = ProjectConfig::load(project_root).ok().flatten()?;
    let transparency = cfg.privacy.transparency.as_deref()?;
    Some(match transparency {
        "on" | "full" | "full_transparency" => crate::core::anchor::TransparencyMode::On,
        _ => crate::core::anchor::TransparencyMode::Off,
    })
}

/// Test-only convenience accessors that load the config from disk.
/// Production code should use `crate::commands::sync::resolve()` instead,
/// which loads the project config once and resolves all settings in a single pass.
#[cfg(test)]
pub fn anchor_remote(project_root: &str) -> Option<String> {
    let cfg = ProjectConfig::load(project_root).ok().flatten()?;
    if cfg.anchors.remote.is_empty() {
        None
    } else {
        Some(cfg.anchors.remote)
    }
}

#[cfg(test)]
pub fn api_key(project_root: &str) -> Option<String> {
    let cfg = ProjectConfig::load(project_root).ok().flatten()?;
    if cfg.server.api_key.is_empty() {
        None
    } else {
        Some(cfg.server.api_key)
    }
}

#[cfg(test)]
pub fn api_url(project_root: &str) -> Option<String> {
    let cfg = ProjectConfig::load(project_root).ok().flatten()?;
    if cfg.server.url.is_empty() {
        None
    } else {
        Some(cfg.server.url)
    }
}

fn default_schema_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}

fn strip_utf8_bom(data: &[u8]) -> &str {
    let stripped = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    std::str::from_utf8(stripped).unwrap_or("")
}

/// Ensure `.oobo/.gitignore` exists so ephemeral data is never committed.
///
/// Only `config` is intended to be version-controlled (shared team settings).
/// Everything else  --  caches, temp files  --  stays local.
fn ensure_gitignore(oobo_dir: &Path) {
    let gi = oobo_dir.join(".gitignore");
    let content = "# Managed by oobo -- do not edit.\n\
         # Only config is intended to be committed.\n\
         # Secrets (API keys) are never committed.\n\
         *\n\
         !.gitignore\n\
         !config\n";
    if gi.exists() {
        // Migrate: ensure secrets is not allowed through
        if let Ok(existing) = std::fs::read_to_string(&gi) {
            if existing.contains("!secrets") {
                let updated = existing.replace("!secrets", "");
                let _ = std::fs::write(&gi, updated);
            }
        }
        return;
    }
    let _ = std::fs::write(gi, content);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_enabled_creates_minimal_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        assert!(set_enabled(&root, "r:github.com/oobo/oobo", true).unwrap());
        let path = path_for(&root);
        assert!(path.exists());

        let cfg = ProjectConfig::load(&root).unwrap().unwrap();
        assert_eq!(cfg.schema_version, 1);
        assert_eq!(cfg.project.id, "r:github.com/oobo/oobo");
        assert!(cfg.project.enabled);
        assert_eq!(cfg.privacy.transparency, None);
        assert!(cfg.privacy.redact);
    }

    #[test]
    fn set_enabled_does_not_overwrite_existing_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let mut cfg = ProjectConfig::for_project("original");
        cfg.privacy.transparency = Some("off".to_string());
        cfg.save(&root).unwrap();

        assert!(!set_enabled(&root, "replacement", true).unwrap());
        let loaded = ProjectConfig::load(&root).unwrap().unwrap();
        assert_eq!(loaded.project.id, "original");
        assert_eq!(loaded.privacy.transparency.as_deref(), Some("off"));
    }

    #[test]
    fn server_section_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        let mut cfg = ProjectConfig::for_project("test");
        cfg.server.api_key = "sk_project_test".to_string();
        cfg.server.url = "https://staging.oobo.ai".to_string();
        cfg.anchors.remote = "oobo".to_string();
        cfg.save(&root).unwrap();

        let loaded = ProjectConfig::load(&root).unwrap().unwrap();
        assert_eq!(loaded.server.api_key, "sk_project_test");
        assert_eq!(loaded.server.url, "https://staging.oobo.ai");
        assert_eq!(loaded.anchors.remote, "oobo");

        assert_eq!(api_key(&root).as_deref(), Some("sk_project_test"));
        assert_eq!(api_url(&root).as_deref(), Some("https://staging.oobo.ai"));
        assert_eq!(anchor_remote(&root).as_deref(), Some("oobo"));
    }

    #[test]
    fn empty_server_section_omitted_in_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        let cfg = ProjectConfig::for_project("test");
        cfg.save(&root).unwrap();

        let raw = std::fs::read_to_string(path_for(&root)).unwrap();
        assert!(
            !raw.contains("[server]"),
            "empty server section should be omitted"
        );
        assert!(api_key(&root).is_none());
        assert!(api_url(&root).is_none());
    }

    #[test]
    fn api_key_stored_in_secrets_not_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        let mut cfg = ProjectConfig::for_project("test");
        cfg.server.api_key = "sk_secret_test".to_string();
        cfg.server.url = "https://api.oobo.ai".to_string();
        cfg.save(&root).unwrap();

        // Config file must NOT contain the key
        let config_raw = std::fs::read_to_string(path_for(&root)).unwrap();
        assert!(
            !config_raw.contains("sk_secret_test"),
            "api_key must not appear in config file"
        );
        // URL should still be in config
        assert!(config_raw.contains("https://api.oobo.ai"));

        // Secrets file must contain the key
        let secrets_raw = std::fs::read_to_string(secrets_path_for(&root)).unwrap();
        assert!(
            secrets_raw.contains("sk_secret_test"),
            "api_key must be in secrets file"
        );

        // Round-trip: load should merge them back
        let loaded = ProjectConfig::load(&root).unwrap().unwrap();
        assert_eq!(loaded.server.api_key, "sk_secret_test");
        assert_eq!(loaded.server.url, "https://api.oobo.ai");
    }

    #[test]
    fn unset_key_removes_secrets_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        // Set a key
        let mut cfg = ProjectConfig::for_project("test");
        cfg.server.api_key = "sk_to_remove".to_string();
        cfg.save(&root).unwrap();
        assert!(secrets_path_for(&root).exists());

        // Unset (clear) the key and save again
        let mut cfg = ProjectConfig::load(&root).unwrap().unwrap();
        cfg.server.api_key.clear();
        cfg.save(&root).unwrap();

        // Secrets file should be gone
        assert!(
            !secrets_path_for(&root).exists(),
            "secrets file must be removed on unset"
        );
        // Load should return no key
        assert!(api_key(&root).is_none());
    }

    #[test]
    fn set_enabled_persists_project_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        assert!(set_enabled(&root, "p:test", false).unwrap());
        assert!(!is_enabled(&root));

        let raw = std::fs::read_to_string(path_for(&root)).unwrap();
        assert!(raw.contains("enabled = false"));

        assert!(set_enabled(&root, "p:test", true).unwrap());
        assert!(is_enabled(&root));

        let raw = std::fs::read_to_string(path_for(&root)).unwrap();
        assert!(
            !raw.contains("enabled = true"),
            "default enabled state should stay omitted"
        );
    }
}
