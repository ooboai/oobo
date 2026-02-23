use std::fs;
use std::path::{Path, PathBuf};

use crate::cursor::transcript::Message;
use crate::cursor::Session;

fn zed_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/Zed"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::data_dir().map(|d| d.join("zed"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn conversations_dir() -> Option<PathBuf> {
    zed_data_dir().map(|d| d.join("conversations"))
}

fn threads_dir() -> Option<PathBuf> {
    zed_data_dir().map(|d| d.join("threads"))
}

fn parse_conversation_file(path: &Path) -> Option<Session> {
    let content = fs::read_to_string(path).ok()?;

    // Zed conversation files can be JSON or JSONL. Try JSON first.
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
        return session_from_json(&data, path);
    }

    // Try JSONL: first line might be metadata
    let first_line = content.lines().next()?;
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(first_line) {
        return session_from_json(&data, path);
    }

    None
}

fn session_from_json(data: &serde_json::Value, path: &Path) -> Option<Session> {
    let session_id = path.file_stem().and_then(|s| s.to_str())?.to_string();

    let title = data
        .get("title")
        .or_else(|| data.get("summary"))
        .and_then(|v| v.as_str())
        .unwrap_or("Zed conversation")
        .to_string();

    let model = data
        .get("model")
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.get("name").and_then(|n| n.as_str()).map(String::from))
        })
        .unwrap_or_default();

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
        name: title,
        mode: model,
        created_at: mtime,
        updated_at: mtime,
        project_path: String::new(),
        workspace_dir: String::new(),
        source: "zed".to_string(),
    })
}

pub fn sessions_for_project(_project_root: &str) -> Result<Vec<Session>, String> {
    // Zed conversations are not workspace-scoped in the filesystem,
    // so we return all and let the caller filter if needed.
    all_sessions()
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let mut sessions = Vec::new();

    for dir_fn in [conversations_dir, threads_dir] {
        if let Some(dir) = dir_fn() {
            if dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            if let Some(s) = parse_conversation_file(&p) {
                                sessions.push(s);
                            }
                        }
                    }
                }
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use super::*;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        for dir_fn in [conversations_dir, threads_dir] {
            if let Some(dir) = dir_fn() {
                if dir.is_dir() {
                    if let Ok(entries) = fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                            if stem == session_id || stem.starts_with(session_id) {
                                return Some(p);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    pub fn count_messages(_project_path: &str, session_id: &str) -> u32 {
        match find_transcript_path("", session_id) {
            Some(p) => parse_messages(&p).len() as u32,
            None => 0,
        }
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let content = fs::read_to_string(path).unwrap_or_default();

        // Try full JSON
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(msgs) = extract_messages_from_json(&data) {
                return msgs;
            }
        }

        // Try JSONL
        let mut messages = Vec::new();
        for line in content.lines() {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(line) {
                let role = data.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                let text = data
                    .get("content")
                    .or_else(|| data.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !text.is_empty() {
                    messages.push(Message {
                        role: role.to_string(),
                        text: text.to_string(),
                    });
                }
            }
        }

        // Try Zed's text-based conversation format: "You:" / "Assistant:" blocks
        if messages.is_empty() {
            messages = parse_text_conversation(&content);
        }

        messages
    }

    fn extract_messages_from_json(data: &serde_json::Value) -> Option<Vec<Message>> {
        let messages_key = data
            .get("messages")
            .or_else(|| data.get("conversation"))
            .or_else(|| data.get("entries"))
            .and_then(|v| v.as_array())?;

        let mut messages = Vec::new();
        for msg in messages_key {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let text = msg
                .get("content")
                .or_else(|| msg.get("text"))
                .or_else(|| msg.get("body"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !text.is_empty() {
                messages.push(Message {
                    role: role.to_string(),
                    text: text.to_string(),
                });
            }
        }

        if messages.is_empty() {
            None
        } else {
            Some(messages)
        }
    }

    pub fn parse_text_conversation(content: &str) -> Vec<Message> {
        let mut messages = Vec::new();
        let mut current_role: Option<&str> = None;
        let mut current_text = String::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "You:" || trimmed == "Human:" || trimmed == "User:" {
                if let Some(role) = current_role.take() {
                    if !current_text.trim().is_empty() {
                        messages.push(Message {
                            role: role.to_string(),
                            text: current_text.trim().to_string(),
                        });
                    }
                }
                current_text.clear();
                current_role = Some("user");
                continue;
            }
            if trimmed == "Assistant:" || trimmed == "AI:" || trimmed == "System:" {
                if let Some(role) = current_role.take() {
                    if !current_text.trim().is_empty() {
                        messages.push(Message {
                            role: role.to_string(),
                            text: current_text.trim().to_string(),
                        });
                    }
                }
                current_text.clear();
                current_role = Some("assistant");
                continue;
            }
            if current_role.is_some() {
                current_text.push_str(line);
                current_text.push('\n');
            }
        }

        if let Some(role) = current_role {
            if !current_text.trim().is_empty() {
                messages.push(Message {
                    role: role.to_string(),
                    text: current_text.trim().to_string(),
                });
            }
        }

        messages
    }

    pub fn read_transcript(path: &Path, max_messages: u32) -> String {
        let messages = parse_messages(path);
        let start = if messages.len() > max_messages as usize {
            messages.len() - max_messages as usize
        } else {
            0
        };
        let mut out = String::new();
        for msg in &messages[start..] {
            let label = if msg.role == "user" {
                "User"
            } else {
                "Assistant"
            };
            out.push_str(&format!("── {label} ──\n{}\n\n", msg.text));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_zed_json_conversation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conv = tmp.path().join("test-conv.json");
        fs::write(
            &conv,
            r#"{
                "title": "Help with Rust",
                "model": "claude-3.5-sonnet",
                "messages": [
                    {"role": "user", "content": "How do I use iterators?"},
                    {"role": "assistant", "content": "Iterators in Rust..."}
                ]
            }"#,
        )
        .unwrap();

        let session = parse_conversation_file(&conv).unwrap();
        assert_eq!(session.name, "Help with Rust");
        assert_eq!(session.source, "zed");

        let msgs = transcript::parse_messages(&conv);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn test_parse_text_conversation() {
        let content = "You:\nHello world\n\nAssistant:\nHi there!\n";
        let msgs = transcript::parse_text_conversation(content);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }
}
