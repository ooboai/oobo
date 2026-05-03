pub mod transcript;

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::tools::cursor::Session;

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
    path.replace(['/', '\\'], "-")
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
    let ws_dir = project_dir.to_string_lossy().to_string();
    let entries = fs::read_dir(&project_dir)
        .map_err(|e| format!("cannot read {}: {e}", project_dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            if let Some(session) = session_from_jsonl(&path, project_root, &ws_dir) {
                sessions.push(session);
            }
        } else if path.is_dir() {
            collect_subagent_sessions(&path, project_root, &ws_dir, &mut sessions);
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
        let ws_dir = dir.to_string_lossy().to_string();

        let dir_entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for file_entry in dir_entries.flatten() {
            let path = file_entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                if let Some(session) = session_from_jsonl(&path, &project_path, &ws_dir) {
                    sessions.push(session);
                }
            } else if path.is_dir() {
                collect_subagent_sessions(&path, &project_path, &ws_dir, &mut sessions);
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

/// Scan a session directory for subagent transcripts.
/// Claude Code stores subagents in `{session-uuid}/subagents/agent-{hash}.jsonl`.
fn collect_subagent_sessions(
    session_dir: &Path,
    project_path: &str,
    ws_dir: &str,
    sessions: &mut Vec<Session>,
) {
    let subagents_dir = session_dir.join("subagents");
    if !subagents_dir.is_dir() {
        return;
    }
    let entries = match fs::read_dir(&subagents_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            if let Some(session) = subagent_session_from_jsonl(&path, project_path, ws_dir) {
                sessions.push(session);
            }
        }
    }
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
    let session_id = path.file_stem()?.to_str()?.to_string();
    if !is_uuid_like(&session_id) {
        return None;
    }
    parse_claude_jsonl(path, &session_id, project_path, ws_dir)
}

/// Extract session metadata from a Claude subagent JSONL file (agent-{hash}.jsonl).
fn subagent_session_from_jsonl(path: &Path, project_path: &str, ws_dir: &str) -> Option<Session> {
    let filename = path.file_stem()?.to_str()?.to_string();
    if !filename.starts_with("agent-") {
        return None;
    }
    parse_claude_jsonl(path, &filename, project_path, ws_dir)
}

fn parse_claude_jsonl(
    path: &Path,
    session_id: &str,
    project_path: &str,
    ws_dir: &str,
) -> Option<Session> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut name = String::new();
    let mut model = String::new();
    let mut created_at: Option<i64> = None;
    let mut updated_at: Option<i64> = None;
    let mut parent_session_id: Option<String> = None;
    let mut subagent_type: Option<String> = None;
    let mut checked_sidechain = false;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Check isSidechain on any entry that has it, not just the first.
        // Claude subagent JSONL entries carry isSidechain+sessionId (parent's
        // UUID) + agentId (subagent hash) on every user/assistant entry.
        if !checked_sidechain {
            if let Some(is_side) = entry.get("isSidechain").and_then(serde_json::Value::as_bool) {
                checked_sidechain = true;
                if is_side {
                    parent_session_id = entry
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .map(std::string::ToString::to_string);
                    subagent_type = entry
                        .get("agentId")
                        .and_then(|v| v.as_str())
                        .map(std::string::ToString::to_string);
                }
            }
        }

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
                    name = crate::utils::truncate_name(text, crate::utils::MAX_SESSION_NAME_LEN);
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
        session_id: session_id.to_string(),
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
        parent_session_id,
        subagent_type,
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
    entry.get("timestamp").and_then(serde_json::Value::as_i64)
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
    fn test_path_to_slug_windows() {
        assert_eq!(
            path_to_slug("C:\\Users\\dev\\project"),
            "C:-Users-dev-project"
        );
        assert_eq!(path_to_slug("D:\\code\\my-app"), "D:-code-my-app");
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
        assert_eq!(
            crate::utils::truncate_name("hello", crate::utils::MAX_SESSION_NAME_LEN),
            "hello"
        );
        let long = "a".repeat(100);
        let truncated = crate::utils::truncate_name(&long, crate::utils::MAX_SESSION_NAME_LEN);
        assert!(truncated.ends_with('…'));
        assert!(truncated.len() <= 64);
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

    #[test]
    fn test_subagent_session_from_jsonl() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("agent-afc15f7.jsonl");
        let parent_uuid = "faa7dc09-0a79-4791-990d-c0aa0e71a3be";

        let content = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"Search for auth code"}},"isSidechain":true,"sessionId":"{parent_uuid}","agentId":"afc15f7","timestamp":"2026-01-12T22:31:39.855Z","cwd":"/tmp/project"}}
{{"type":"assistant","message":{{"model":"claude-sonnet-4-20251001","type":"message","role":"assistant","content":[{{"type":"text","text":"Found it."}}]}},"isSidechain":true,"sessionId":"{parent_uuid}","agentId":"afc15f7","timestamp":"2026-01-12T22:32:00.000Z"}}"#
        );
        std::fs::write(&path, content).unwrap();

        let session = subagent_session_from_jsonl(&path, "/tmp/project", "/tmp").unwrap();
        assert_eq!(session.session_id, "agent-afc15f7");
        assert_eq!(session.parent_session_id.as_deref(), Some(parent_uuid));
        assert_eq!(session.subagent_type.as_deref(), Some("afc15f7"));
        assert!(session.is_subagent());
        assert_eq!(session.source, "claude");
    }

    #[test]
    fn test_subagent_rejected_without_agent_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("not-an-agent.jsonl");
        std::fs::write(&path, r#"{"type":"user","message":{"content":"hi"}}"#).unwrap();

        assert!(subagent_session_from_jsonl(&path, "/tmp", "/tmp").is_none());
    }

    #[test]
    fn test_collect_subagent_sessions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let parent_uuid = "faa7dc09-0a79-4791-990d-c0aa0e71a3be";
        let session_dir = tmp.path().join(parent_uuid);
        let subagents_dir = session_dir.join("subagents");
        std::fs::create_dir_all(&subagents_dir).unwrap();

        let agent_file = subagents_dir.join("agent-abc1234.jsonl");
        let content = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"task"}},"isSidechain":true,"sessionId":"{parent_uuid}","agentId":"abc1234","timestamp":"2026-01-12T22:31:39.855Z","cwd":"/tmp"}}
{{"type":"assistant","message":{{"model":"claude-sonnet-4-20251001","role":"assistant","content":[{{"type":"text","text":"done"}}]}},"timestamp":"2026-01-12T22:32:00.000Z"}}"#
        );
        std::fs::write(&agent_file, content).unwrap();

        let mut sessions = Vec::new();
        collect_subagent_sessions(&session_dir, "/tmp", "/tmp", &mut sessions);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "agent-abc1234");
        assert_eq!(sessions[0].parent_session_id.as_deref(), Some(parent_uuid));
        assert!(sessions[0].is_subagent());
    }

    #[test]
    fn test_sidechain_detection_skips_metadata_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("agent-test123.jsonl");
        let parent_uuid = "11111111-2222-3333-4444-555555555555";

        // First entry is metadata (no isSidechain), second has it.
        let content = format!(
            r#"{{"type":"file-history-snapshot","messageId":"snap1"}}
{{"type":"user","message":{{"role":"user","content":"hello"}},"isSidechain":true,"sessionId":"{parent_uuid}","agentId":"test123","timestamp":"2026-01-12T22:31:39.855Z","cwd":"/tmp"}}
{{"type":"assistant","message":{{"model":"claude-sonnet-4-20251001","role":"assistant","content":[{{"type":"text","text":"hi"}}]}},"timestamp":"2026-01-12T22:32:00.000Z"}}"#
        );
        std::fs::write(&path, content).unwrap();

        let session = subagent_session_from_jsonl(&path, "/tmp", "/tmp").unwrap();
        assert_eq!(session.parent_session_id.as_deref(), Some(parent_uuid));
        assert_eq!(session.subagent_type.as_deref(), Some("test123"));
    }
}
