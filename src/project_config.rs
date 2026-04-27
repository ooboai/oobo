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
pub struct AnchorsSection {
    /// Optional Git remote target for the anchor metadata branch.
    ///
    /// Defaults to `origin`. May be a configured Git remote name (`oobo`) or a
    /// full Git URL (`git@github.com:org/repo-oobo.git`).
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
        .map(|cfg| cfg.project.enabled)
        .unwrap_or(false)
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

pub fn anchor_remote(project_root: &str) -> Option<String> {
    let cfg = ProjectConfig::load(project_root).ok().flatten()?;
    if cfg.anchors.remote.is_empty() {
        None
    } else {
        Some(cfg.anchors.remote)
    }
}

fn default_schema_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn strip_utf8_bom(data: &[u8]) -> &str {
    let stripped = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    std::str::from_utf8(stripped).unwrap_or("")
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
