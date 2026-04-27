use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ADAPTERS_DIR: &str = "adapters";
const MANIFEST_EXT: &str = "toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalAdapterManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<ExternalAdapterCapability>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAdapterCapability {
    ListSessions,
    ReadTranscript,
    TurnStream,
    NativeStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExternalAdapterRequest {
    Handshake {
        protocol_version: u32,
        project_root: String,
    },
    ListSessions {
        project_root: String,
    },
    ReadTranscript {
        project_root: String,
        session_id: String,
    },
    TurnStream {
        project_root: String,
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExternalAdapterResponse {
    Handshake {
        adapter_id: String,
        protocol_version: u32,
        capabilities: Vec<ExternalAdapterCapability>,
    },
    Sessions {
        sessions: Vec<ExternalSession>,
    },
    Transcript {
        session_id: String,
        content: serde_json::Value,
    },
    Turns {
        session_id: String,
        turns: Vec<serde_json::Value>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSession {
    pub session_id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

impl ExternalAdapterManifest {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!(
                "invalid adapter id '{}': expected ASCII letters, digits, '_' or '.'",
                self.id
            ));
        }
        if self.display_name.trim().is_empty() {
            return Err("adapter display_name must not be empty".to_string());
        }
        if self.command.is_empty() || self.command.iter().any(|part| part.trim().is_empty()) {
            return Err("adapter command must contain at least one non-empty part".to_string());
        }
        if self.capabilities.is_empty() {
            return Err("adapter capabilities must not be empty".to_string());
        }
        Ok(())
    }
}

pub fn manifest_dir(project_root: &str) -> PathBuf {
    crate::project_config::path_for(project_root)
        .parent()
        .unwrap_or_else(|| Path::new(project_root))
        .join(ADAPTERS_DIR)
}

pub fn manifest_path(project_root: &str, adapter_id: &str) -> Result<PathBuf, String> {
    if !valid_id(adapter_id) {
        return Err(format!(
            "invalid adapter id '{adapter_id}': expected ASCII letters, digits, '_' or '.'"
        ));
    }
    Ok(manifest_dir(project_root).join(format!("{adapter_id}.{MANIFEST_EXT}")))
}

pub fn load_project_manifests(project_root: &str) -> Result<Vec<ExternalAdapterManifest>, String> {
    let dir = manifest_dir(project_root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut manifests = Vec::new();
    for entry in
        std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| format!("cannot read adapter manifest entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(MANIFEST_EXT) {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let manifest: ExternalAdapterManifest = toml::from_str(&text)
            .map_err(|e| format!("invalid adapter manifest {}: {e}", path.display()))?;
        manifest.validate()?;
        manifests.push(manifest);
    }
    manifests.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(manifests)
}

pub fn load_project_manifest(
    project_root: &str,
    adapter_id: &str,
) -> Result<Option<ExternalAdapterManifest>, String> {
    Ok(load_project_manifests(project_root)?
        .into_iter()
        .find(|manifest| manifest.id == adapter_id))
}

pub fn run_adapter_request(
    manifest: &ExternalAdapterManifest,
    request: &ExternalAdapterRequest,
) -> Result<ExternalAdapterResponse, String> {
    manifest.validate()?;
    let mut command = Command::new(&manifest.command[0]);
    command
        .args(manifest.command.iter().skip(1))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, value) in &manifest.env {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn external adapter '{}': {e}", manifest.id))?;
    let line = serde_json::to_string(request)
        .map_err(|e| format!("serialize external adapter request: {e}"))?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| format!("external adapter '{}' stdin unavailable", manifest.id))?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| format!("write external adapter request: {e}"))?;
    }
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait for external adapter '{}': {e}", manifest.id))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err(format!(
                "external adapter '{}' exited with {}",
                manifest.id, output.status
            ));
        }
        return Err(format!(
            "external adapter '{}' exited with {}: {stderr}",
            manifest.id, output.status
        ));
    }

    serde_json::from_slice::<ExternalAdapterResponse>(&output.stdout).map_err(|e| {
        let stdout = String::from_utf8_lossy(&output.stdout);
        format!(
            "parse external adapter '{}' response: {e}: {stdout}",
            manifest.id
        )
    })
}

fn default_schema_version() -> u32 {
    1
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ExternalAdapterManifest {
        ExternalAdapterManifest {
            schema_version: 1,
            id: "acme.custom".to_string(),
            display_name: "Acme Custom".to_string(),
            command: vec!["/opt/acme/adapter".to_string(), "--stdio".to_string()],
            capabilities: vec![
                ExternalAdapterCapability::ListSessions,
                ExternalAdapterCapability::TurnStream,
            ],
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn manifest_validation_accepts_explicit_command() {
        assert!(manifest().validate().is_ok());
    }

    #[test]
    fn manifest_validation_rejects_magic_or_shell_like_ids() {
        let mut m = manifest();
        m.id = "acme-custom".to_string();
        assert!(m.validate().is_err());
    }

    #[test]
    fn request_response_shapes_are_tagged_json() {
        let request = ExternalAdapterRequest::ReadTranscript {
            project_root: "/repo".to_string(),
            session_id: "s1".to_string(),
        };
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["type"], "read_transcript");
        assert_eq!(json["session_id"], "s1");
    }

    #[test]
    fn load_project_manifests_reads_sorted_toml_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let dir = manifest_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();

        let a = manifest();
        let mut b = manifest();
        b.id = "acme.alpha".to_string();
        b.display_name = "Acme Alpha".to_string();

        std::fs::write(dir.join("z.toml"), toml::to_string_pretty(&a).unwrap()).unwrap();
        std::fs::write(dir.join("a.toml"), toml::to_string_pretty(&b).unwrap()).unwrap();

        let loaded = load_project_manifests(&root).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "acme.alpha");
        assert_eq!(loaded[1].id, "acme.custom");
    }

    #[test]
    fn load_project_manifest_finds_one_adapter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let dir = manifest_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();

        let m = manifest();
        std::fs::write(
            dir.join("adapter.toml"),
            toml::to_string_pretty(&m).unwrap(),
        )
        .unwrap();

        let loaded = load_project_manifest(&root, "acme.custom")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.id, "acme.custom");
        assert!(load_project_manifest(&root, "missing.adapter")
            .unwrap()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn run_adapter_request_invokes_manifest_command() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("adapter.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
read request
printf '%s\n' '{"type":"handshake","adapter_id":"acme.custom","protocol_version":1,"capabilities":["list_sessions"]}'
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let mut manifest = manifest();
        manifest.command = vec![script.to_string_lossy().to_string()];
        let response = run_adapter_request(
            &manifest,
            &ExternalAdapterRequest::Handshake {
                protocol_version: 1,
                project_root: "/repo".to_string(),
            },
        )
        .unwrap();

        match response {
            ExternalAdapterResponse::Handshake {
                adapter_id,
                protocol_version,
                capabilities,
            } => {
                assert_eq!(adapter_id, "acme.custom");
                assert_eq!(protocol_version, 1);
                assert_eq!(capabilities, vec![ExternalAdapterCapability::ListSessions]);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }
}
