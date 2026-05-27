use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const PROJECT_CONFIG_DIR: &str = ".oobo";
const PROJECT_CONFIG_FILE: &str = "config";

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
        toml::from_str(text)
            .map(Some)
            .map_err(|e| format!("invalid project config at {}: {e}", path.display()))
    }

    pub fn save(&self, project_root: &str) -> Result<(), String> {
        let path = path_for(project_root);
        let dir = path
            .parent()
            .ok_or_else(|| format!("invalid project config path {}", path.display()))?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

        ensure_gitignore(dir);

        let content = toml::to_string_pretty(self)
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
    if gi.exists() {
        return;
    }
    let _ = std::fs::write(
        gi,
        "# Managed by oobo  --  do not edit.\n\
         # Only config is intended to be committed.\n\
         *\n\
         !.gitignore\n\
         !config\n",
    );
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
