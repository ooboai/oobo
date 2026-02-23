pub mod transcript;

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::cursor::Session;

/// Directory where Claude Code stores its data (`~/.claude`).
pub fn claude_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
}

/// Directory where Claude Code stores per-project session data.
pub fn claude_projects_dir() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("projects"))
}

/// Convert a filesystem path to Claude's project directory slug.
/// `/home/user/projects/my-app` → `-home-user-projects-my-app`
pub fn path_to_slug(path: &str) -> String {
    path.replace('/', "-")
}

/// Get all sessions for a given project root.
pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let projects_dir = claude_projects_dir().ok_or("Claude data directory not found")?;
    let slug = path_to_slug(project_root);
    let project_dir = projects_dir.join(&slug);

    if !project_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let entries = fs::read_dir(&project_dir)
        .map_err(|e| format!("cannot read {}: {e}", project_dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            if let Some(session) =
                session_from_jsonl(&path, project_root, &project_dir.to_string_lossy())
            {
                sessions.push(session);
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

/// Get all sessions across all Claude projects.
pub fn all_sessions() -> Result<Vec<Session>, String> {
    let projects_dir = claude_projects_dir().ok_or("Claude data directory not found")?;
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let entries = fs::read_dir(&projects_dir)
        .map_err(|e| format!("cannot read {}: {e}", projects_dir.display()))?;

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let project_path = project_path_from_dir(&dir).unwrap_or_default();

        let dir_entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for file_entry in dir_entries.flatten() {
            let path = file_entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                if let Some(session) =
                    session_from_jsonl(&path, &project_path, &dir.to_string_lossy())
                {
                    sessions.push(session);
                }
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

/// Find a Claude session by ID prefix.
#[allow(dead_code)]
pub fn find_session(id_prefix: &str) -> Result<Session, String> {
    let all = all_sessions()?;
    all.into_iter()
        .find(|s| s.session_id.starts_with(id_prefix))
        .ok_or_else(|| format!("Claude session not found: {id_prefix}"))
}

/// Read the project path from the first JSONL entry's `cwd` field.
fn project_path_from_dir(dir: &Path) -> Option<String> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            if let Some(cwd) = first_cwd_from_jsonl(&path) {
                return Some(cwd);
            }
        }
    }
    None
}

fn first_cwd_from_jsonl(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(cwd) = entry.get("cwd").and_then(|v| v.as_str()) {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// Extract session metadata from a Claude session JSONL file.
fn session_from_jsonl(path: &Path, project_path: &str, ws_dir: &str) -> Option<Session> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let session_id = path.file_stem()?.to_str()?.to_string();
    if !is_uuid_like(&session_id) {
        return None;
    }

    let mut name = String::new();
    let mut model = String::new();
    let mut created_at: Option<i64> = None;
    let mut updated_at: Option<i64> = None;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if let Some(ms) = parse_timestamp(&entry) {
            if created_at.is_none() {
                created_at = Some(ms);
            }
            updated_at = Some(ms);
        }

        if entry_type == "user" && name.is_empty() {
            if let Some(content) = entry.get("message").and_then(|m| m.get("content")) {
                if let Some(text) = content.as_str() {
                    name = truncate_name(text);
                }
            }
        }

        if entry_type == "assistant" && model.is_empty() {
            if let Some(m) = entry
                .get("message")
                .and_then(|m| m.get("model"))
                .and_then(|v| v.as_str())
            {
                model = format_model_name(m);
            }
        }
    }

    Some(Session {
        session_id,
        name,
        mode: if model.is_empty() {
            "claude".to_string()
        } else {
            model
        },
        created_at,
        updated_at,
        project_path: project_path.to_string(),
        workspace_dir: ws_dir.to_string(),
        source: "claude".to_string(),
    })
}

fn is_uuid_like(s: &str) -> bool {
    s.len() >= 32 && s.contains('-') && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn parse_timestamp(entry: &serde_json::Value) -> Option<i64> {
    if let Some(ts_str) = entry.get("timestamp").and_then(|v| v.as_str()) {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
            return Some(dt.timestamp_millis());
        }
    }
    entry.get("timestamp").and_then(|v| v.as_i64())
}

fn truncate_name(text: &str) -> String {
    let cleaned = text.trim();
    if cleaned.len() <= 60 {
        cleaned.to_string()
    } else {
        let truncated: String = cleaned.chars().take(57).collect();
        format!("{truncated}...")
    }
}

fn format_model_name(model: &str) -> String {
    if model.contains("opus") {
        if model.contains("4-5") {
            "opus-4.5".to_string()
        } else {
            "opus".to_string()
        }
    } else if model.contains("sonnet") {
        if model.contains("4-5") {
            "sonnet-4.5".to_string()
        } else {
            "sonnet".to_string()
        }
    } else if model.contains("haiku") {
        "haiku".to_string()
    } else {
        model.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_slug() {
        assert_eq!(
            path_to_slug("/home/user/projects/my-app"),
            "-home-user-projects-my-app"
        );
        assert_eq!(path_to_slug("/tmp"), "-tmp");
    }

    #[test]
    fn test_is_uuid_like() {
        assert!(is_uuid_like("faa7dc09-0a79-4791-990d-c0aa0e71a3be"));
        assert!(is_uuid_like("46ffc4ee-66eb-4ed7-986a-258032ec4d7c"));
        assert!(!is_uuid_like("agent-a26b647"));
        assert!(!is_uuid_like("short"));
    }

    #[test]
    fn test_truncate_name() {
        assert_eq!(truncate_name("hello"), "hello");
        let long = "a".repeat(100);
        let truncated = truncate_name(&long);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() <= 60);
    }

    #[test]
    fn test_format_model_name() {
        assert_eq!(format_model_name("claude-opus-4-5-20251101"), "opus-4.5");
        assert_eq!(
            format_model_name("claude-sonnet-4-5-20260101"),
            "sonnet-4.5"
        );
        assert_eq!(format_model_name("claude-sonnet-4-20251001"), "sonnet");
        assert_eq!(format_model_name("claude-haiku-3-20240307"), "haiku");
        assert_eq!(format_model_name("gpt-4"), "gpt-4");
    }

    #[test]
    fn test_session_from_jsonl() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = "faa7dc09-0a79-4791-990d-c0aa0e71a3be";
        let path = tmp.path().join(format!("{session_id}.jsonl"));

        let content = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"what is this project about ?"}},"uuid":"abc","sessionId":"{session_id}","timestamp":"2026-01-12T22:31:39.855Z","cwd":"/tmp/project"}}
{{"type":"assistant","message":{{"model":"claude-opus-4-5-20251101","type":"message","role":"assistant","content":[{{"type":"text","text":"This is a test."}}]}},"uuid":"def","sessionId":"{session_id}","timestamp":"2026-01-12T22:32:00.000Z"}}"#
        );
        std::fs::write(&path, content).unwrap();

        let session = session_from_jsonl(&path, "/tmp/project", "/tmp").unwrap();
        assert_eq!(session.session_id, session_id);
        assert_eq!(session.name, "what is this project about ?");
        assert_eq!(session.mode, "opus-4.5");
        assert_eq!(session.source, "claude");
        assert!(session.created_at.is_some());
        assert!(session.updated_at.is_some());
    }

    #[test]
    fn test_session_from_jsonl_not_uuid() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("agent-a26b647.jsonl");
        std::fs::write(&path, "{}").unwrap();

        assert!(session_from_jsonl(&path, "/tmp", "/tmp").is_none());
    }
}
