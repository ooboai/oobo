/// Amp (Sourcegraph) — ACP-based agent, thread JSON files in ~/.local/share/amp/threads/.
///
/// Session storage:
///   macOS/Linux: ~/.local/share/amp/threads/<thread-id>.json
///   Each file contains thread metadata + message history.
use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::cursor::Session;

pub fn amp_threads_dir() -> Option<PathBuf> {
    // Amp uses XDG-style data dir on all platforms
    if let Some(home) = dirs::home_dir() {
        let xdg_path = home.join(".local/share/amp/threads");
        if xdg_path.exists() {
            return Some(xdg_path);
        }
    }
    // Also check XDG data dir
    if let Some(data) = dirs::data_local_dir() {
        let p = data.join("amp/threads");
        if p.exists() {
            return Some(p);
        }
    }
    // macOS: might also be in ~/Library/Application Support/amp/threads
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            let p = home.join("Library/Application Support/amp/threads");
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn session_from_thread_file(path: &Path) -> Option<Session> {
    let session_id = path.file_stem().and_then(|s| s.to_str())?.to_string();
    let content = fs::read_to_string(path).ok()?;
    let data: serde_json::Value = serde_json::from_str(&content).ok()?;

    let name = data
        .get("title")
        .or_else(|| data.get("name"))
        .or_else(|| data.get("summary"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty()).map_or_else(|| "Amp thread".to_string(), |s| crate::utils::truncate_name(s, crate::utils::MAX_SESSION_NAME_LEN));

    let project_path = data
        .get("workingDirectory")
        .or_else(|| data.get("cwd"))
        .or_else(|| data.get("directory"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let created_at = data
        .get("createdAt")
        .or_else(|| data.get("created_at"))
        .and_then(serde_json::Value::as_i64);

    let updated_at = data
        .get("updatedAt")
        .or_else(|| data.get("updated_at"))
        .and_then(serde_json::Value::as_i64);

    let mtime = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64
        });

    Some(Session {
        session_id,
        name,
        mode: "amp".to_string(),
        created_at: created_at.or(mtime),
        updated_at: updated_at.or(mtime),
        project_path,
        workspace_dir: path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        source: "amp".to_string(),
        parent_session_id: None,
        subagent_type: None,
    })
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let dir = match amp_threads_dir() {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };

    let mut sessions = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|e| e == "json") {
                if let Some(s) = session_from_thread_file(&p) {
                    sessions.push(s);
                }
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let norm_root = crate::paths::normalize_path(project_root);
    let all = all_sessions()?;
    let mut sessions: Vec<Session> = all
        .into_iter()
        .filter(|s| {
            !s.project_path.is_empty() && crate::paths::normalize_path(&s.project_path) == norm_root
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::core::message::Message;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        let dir = super::amp_threads_dir()?;
        let p = dir.join(format!("{session_id}.json"));
        if p.exists() {
            return Some(p);
        }
        None
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let data: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let msgs = data
            .get("messages")
            .or_else(|| data.get("turns"))
            .or_else(|| data.get("conversation"))
            .and_then(|v| v.as_array());

        let mut messages = Vec::new();
        if let Some(arr) = msgs {
            for msg in arr {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                let text = msg
                    .get("content")
                    .or_else(|| msg.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.is_empty() {
                    messages.push(Message {
                        role: role.to_string(),
                        text,
                        timestamp_ms: None,
                    });
                }
            }
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_amp_thread() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("thread-abc.json");
        fs::write(
            &path,
            r#"{
                "title": "Fix authentication bug",
                "workingDirectory": "/home/user/myapp",
                "createdAt": 1700000000000,
                "updatedAt": 1700001000000,
                "messages": [
                    {"role": "user", "content": "Can you help me fix this auth bug?"},
                    {"role": "assistant", "content": "Sure! Let me look at the code."}
                ]
            }"#,
        )
        .unwrap();

        let session = session_from_thread_file(&path).unwrap();
        assert_eq!(session.session_id, "thread-abc");
        assert_eq!(session.name, "Fix authentication bug");
        assert_eq!(session.project_path, "/home/user/myapp");
        assert_eq!(session.source, "amp");

        let msgs = transcript::parse_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn test_no_threads_dir_returns_empty() {
        let result = all_sessions();
        assert!(result.is_ok());
    }
}
