use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::cursor::transcript::Message;
use crate::tools::cursor::Session;
use crate::tools::vscode_fork;

/// VS Code app name (standard VS Code installation).
const VSCODE_APP: &str = "Code";

fn chat_sessions_in_workspace(ws_dir: &Path) -> Vec<PathBuf> {
    let chat_dir = ws_dir.join("chatSessions");
    if !chat_dir.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(&chat_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "json" || ext == "jsonl" {
                files.push(p);
            }
        }
    }
    files
}

/// Replay a JSONL mutation log into a full session JSON object.
/// Format: kind=0 is base state, kind=1 is set, kind=2 is array push.
fn replay_jsonl(content: &str) -> Option<serde_json::Value> {
    let mut state: Option<serde_json::Value> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(line).ok()?;
        let kind = entry.get("kind").and_then(|k| k.as_u64()).unwrap_or(99);

        match kind {
            0 => {
                state = entry.get("v").cloned();
            }
            1 => {
                // Set mutation: k is key path, v is value
                if let (Some(keys), Some(val), Some(ref mut s)) =
                    (entry.get("k"), entry.get("v"), &mut state)
                {
                    if let Some(key_arr) = keys.as_array() {
                        set_at_path(s, key_arr, val.clone());
                    }
                }
            }
            2 => {
                // Array splice/push: k is key path, v is array of items to append
                if let (Some(keys), Some(val), Some(ref mut s)) =
                    (entry.get("k"), entry.get("v"), &mut state)
                {
                    if let Some(key_arr) = keys.as_array() {
                        if let Some(items) = val.as_array() {
                            push_at_path(s, key_arr, items);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    state
}

fn set_at_path(root: &mut serde_json::Value, keys: &[serde_json::Value], val: serde_json::Value) {
    if keys.len() == 1 {
        if let Some(key) = keys[0].as_str() {
            if let Some(obj) = root.as_object_mut() {
                obj.insert(key.to_string(), val);
            }
        }
    } else if !keys.is_empty() {
        if let Some(key) = keys[0].as_str() {
            if let Some(obj) = root.as_object_mut() {
                let child = obj
                    .entry(key)
                    .or_insert(serde_json::Value::Object(serde_json::Map::new()));
                set_at_path(child, &keys[1..], val);
            }
        }
    }
}

fn push_at_path(
    root: &mut serde_json::Value,
    keys: &[serde_json::Value],
    items: &[serde_json::Value],
) {
    if keys.len() == 1 {
        if let Some(key) = keys[0].as_str() {
            if let Some(obj) = root.as_object_mut() {
                let arr = obj
                    .entry(key)
                    .or_insert(serde_json::Value::Array(Vec::new()));
                if let Some(target) = arr.as_array_mut() {
                    for item in items {
                        target.push(item.clone());
                    }
                }
            }
        }
    } else if !keys.is_empty() {
        if let Some(key) = keys[0].as_str() {
            if let Some(obj) = root.as_object_mut() {
                if let Some(child) = obj.get_mut(key) {
                    push_at_path(child, &keys[1..], items);
                }
            }
        }
    }
}

fn parse_session_file(path: &Path, project_path: &str, ws_dir: &str) -> Option<Session> {
    let content = fs::read_to_string(path).ok()?;

    let data: serde_json::Value = if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
        replay_jsonl(&content)?
    } else {
        serde_json::from_str(&content).ok()?
    };

    let session_id = data.get("sessionId")?.as_str()?.to_string();
    let created_at = data.get("creationDate").and_then(|v| v.as_i64());

    let requests = data.get("requests").and_then(|v| v.as_array())?;

    let name = requests
        .first()
        .and_then(|r| r.get("message"))
        .and_then(|m| m.get("text"))
        .and_then(|t| t.as_str())
        .map(|s| crate::utils::truncate_name(s, crate::utils::MAX_SESSION_NAME_LEN))
        .unwrap_or_else(|| "Copilot chat".to_string());

    let model = requests
        .first()
        .and_then(|r| r.get("modelId"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let updated_at = requests
        .last()
        .and_then(|r| r.get("timestamp"))
        .and_then(|v| v.as_i64());

    Some(Session {
        session_id,
        name,
        mode: model,
        created_at,
        updated_at,
        project_path: project_path.to_string(),
        workspace_dir: ws_dir.to_string(),
        source: "copilot".to_string(),
    })
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let ws_dirs = vscode_fork::find_workspace_dirs_for_project(VSCODE_APP, project_root)?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        for file in chat_sessions_in_workspace(ws_dir) {
            if let Some(s) = parse_session_file(&file, folder_path, &ws_dir.to_string_lossy()) {
                sessions.push(s);
            }
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let ws_dirs = vscode_fork::find_all_workspace_dirs(VSCODE_APP)?;
    let mut sessions = Vec::new();
    for (ws_dir, folder_path) in &ws_dirs {
        for file in chat_sessions_in_workspace(ws_dir) {
            if let Some(s) = parse_session_file(&file, folder_path, &ws_dir.to_string_lossy()) {
                sessions.push(s);
            }
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use super::*;

    pub fn find_transcript_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
        let ws_dirs =
            vscode_fork::find_workspace_dirs_for_project(VSCODE_APP, project_path).ok()?;
        for (ws_dir, _) in &ws_dirs {
            for ext in &["json", "jsonl"] {
                let path = ws_dir
                    .join("chatSessions")
                    .join(format!("{session_id}.{ext}"));
                if path.exists() {
                    return Some(path);
                }
            }
        }
        let all_dirs = vscode_fork::find_all_workspace_dirs(VSCODE_APP).ok()?;
        for (ws_dir, _) in &all_dirs {
            for ext in &["json", "jsonl"] {
                let path = ws_dir
                    .join("chatSessions")
                    .join(format!("{session_id}.{ext}"));
                if path.exists() {
                    return Some(path);
                }
            }
        }
        None
    }

    pub fn count_messages(project_path: &str, session_id: &str) -> u32 {
        match find_transcript_path(project_path, session_id) {
            Some(p) => {
                let content = fs::read_to_string(&p).unwrap_or_default();
                let data: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
                data.get("requests")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len() as u32 * 2)
                    .unwrap_or(0)
            }
            None => 0,
        }
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let content = fs::read_to_string(path).unwrap_or_default();
        let data: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let requests = match data.get("requests").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return Vec::new(),
        };

        let mut messages = Vec::new();
        for req in requests {
            if let Some(text) = req
                .get("message")
                .and_then(|m| m.get("text"))
                .and_then(|t| t.as_str())
            {
                messages.push(Message {
                    role: "user".to_string(),
                    text: text.to_string(),
                    timestamp_ms: None,
                });
            }

            let response_text = extract_response_text(req);
            if !response_text.is_empty() {
                messages.push(Message {
                    role: "assistant".to_string(),
                    text: response_text,
                    timestamp_ms: None,
                });
            }
        }

        messages
    }

    fn extract_response_text(req: &serde_json::Value) -> String {
        if let Some(resp) = req.get("response") {
            if let Some(text) = resp.get("value").and_then(|v| v.as_str()) {
                return text.to_string();
            }
            if let Some(result) = resp.get("result") {
                if let Some(text) = result.get("value").and_then(|v| v.as_str()) {
                    return text.to_string();
                }
                if let Some(text) = result.get("message").and_then(|v| v.as_str()) {
                    return text.to_string();
                }
            }
            if let Some(parts) = resp.get("value").and_then(|v| v.as_array()) {
                let texts: Vec<&str> = parts
                    .iter()
                    .filter_map(|p| p.get("value").and_then(|v| v.as_str()))
                    .collect();
                if !texts.is_empty() {
                    return texts.join("\n");
                }
            }
        }
        String::new()
    }

    pub fn extract_stats(path: &Path) -> Option<crate::remote::payload::SessionStats> {
        let content = fs::read_to_string(path).ok()?;
        let data: serde_json::Value = serde_json::from_str(&content).ok()?;
        let requests = data.get("requests").and_then(|v| v.as_array())?;

        let model = requests
            .first()
            .and_then(|r| r.get("modelId"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let first_ts = data.get("creationDate").and_then(|v| v.as_i64());
        let last_ts = requests
            .last()
            .and_then(|r| r.get("timestamp"))
            .and_then(|v| v.as_i64());

        let duration_secs = match (first_ts, last_ts) {
            (Some(f), Some(l)) if l > f => Some(((l - f) / 1000) as u64),
            _ => None,
        };

        Some(crate::remote::payload::SessionStats {
            model,
            input_tokens: None,
            output_tokens: None,
            duration_secs,
            files_touched: Vec::new(),
            tool_call_count: 0,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            is_estimated: false,
            token_source: None,
        })
    }

    pub fn stats_for_session(
        project_path: &str,
        session_id: &str,
    ) -> Option<crate::remote::payload::SessionStats> {
        let path = find_transcript_path(project_path, session_id)?;
        extract_stats(&path)
    }

    pub fn read_transcript(path: &Path, max_messages: u32) -> String {
        let messages = parse_messages(path);
        crate::utils::format_transcript(&messages, max_messages, "Assistant")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_copilot_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_file = tmp.path().join("test-session.json");
        fs::write(
            &session_file,
            r#"{
                "sessionId": "abc-123",
                "creationDate": 1700000000000,
                "version": 3,
                "requests": [
                    {
                        "requestId": "r1",
                        "timestamp": 1700000001000,
                        "modelId": "copilot/gpt-4",
                        "message": {"text": "Hello copilot"},
                        "response": {"value": "Hi there!"}
                    }
                ]
            }"#,
        )
        .unwrap();

        let session = parse_session_file(&session_file, "/tmp/project", "/tmp/ws").unwrap();
        assert_eq!(session.session_id, "abc-123");
        assert_eq!(session.name, "Hello copilot");
        assert_eq!(session.source, "copilot");

        let msgs = transcript::parse_messages(&session_file);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello copilot");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "Hi there!");
    }
}
