use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::cursor::transcript::Message;
use crate::tools::cursor::Session;

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
                        let ext = p.extension().and_then(|e| e.to_str());
                        if p.is_file() && matches!(ext, Some("json" | "jsonl")) {
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

        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(msgs) = extract_messages_from_json(&data) {
                return msgs;
            }
        }

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
                        timestamp_ms: None,
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
                    timestamp_ms: None,
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
                            timestamp_ms: None,
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
                            timestamp_ms: None,
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
                    timestamp_ms: None,
                });
            }
        }

        messages
    }

    pub fn read_transcript(path: &Path, max_messages: u32) -> String {
        let messages = parse_messages(path);
        crate::utils::format_transcript(&messages, max_messages, "Assistant")
    }
}

pub mod telemetry {
    use std::collections::HashMap;
    use std::fs;
    use std::io::BufRead;
    use std::path::PathBuf;

    use crate::analytics::NativeStats;

    /// Path to Zed's telemetry log.
    pub fn telemetry_log_path() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            dirs::home_dir().map(|h| h.join("Library/Logs/Zed/telemetry.log"))
        }
        #[cfg(target_os = "linux")]
        {
            dirs::home_dir().map(|h| h.join(".local/share/zed/logs/telemetry.log"))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            None
        }
    }

    pub fn has_telemetry_log() -> bool {
        telemetry_log_path()
            .map(|p| p.exists() && fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false))
            .unwrap_or(false)
    }

    struct UsageEvent {
        thread_id: String,
        prompt_id: String,
        model: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    }

    struct TurnEvent {
        session_id: String,
        turn_time_ms: u64,
    }

    fn load_telemetry() -> (Vec<UsageEvent>, Vec<TurnEvent>) {
        let path = match telemetry_log_path() {
            Some(p) if p.exists() => p,
            _ => return (Vec::new(), Vec::new()),
        };

        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return (Vec::new(), Vec::new()),
        };

        let reader = std::io::BufReader::new(file);
        let mut usage_events = Vec::new();
        let mut turn_events = Vec::new();

        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event_type = entry
                .get("event_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let props = match entry.get("event_properties") {
                Some(p) => p,
                None => continue,
            };

            match event_type {
                "Agent Thread Completion Usage Updated" => {
                    let thread_id = props
                        .get("thread_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let prompt_id = props
                        .get("prompt_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if thread_id.is_empty() || prompt_id.is_empty() {
                        continue;
                    }
                    usage_events.push(UsageEvent {
                        thread_id,
                        prompt_id,
                        model: props
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(strip_provider),
                        input_tokens: props
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        output_tokens: props
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cache_read_tokens: props
                            .get("cache_read_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cache_creation_tokens: props
                            .get("cache_creation_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                    });
                }
                "Agent Turn Completed" => {
                    let sid = props
                        .get("session")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !sid.is_empty() {
                        turn_events.push(TurnEvent {
                            session_id: sid,
                            turn_time_ms: props
                                .get("turn_time_ms")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                        });
                    }
                }
                _ => {}
            }
        }

        (usage_events, turn_events)
    }

    /// Strip the "zed.dev/" provider prefix from model names.
    fn strip_provider(model: &str) -> String {
        model.strip_prefix("zed.dev/").unwrap_or(model).to_string()
    }

    /// Extract native stats for a specific Zed session/thread.
    pub fn extract_native_stats(session_id: &str) -> Option<NativeStats> {
        if !has_telemetry_log() {
            return None;
        }

        let (usage_events, turn_events) = load_telemetry();

        // Take the last usage event per prompt_id (they report cumulative values)
        let mut last_per_prompt: HashMap<String, &UsageEvent> = HashMap::new();
        for event in &usage_events {
            if event.thread_id == session_id {
                last_per_prompt.insert(event.prompt_id.clone(), event);
            }
        }

        if last_per_prompt.is_empty() {
            return None;
        }

        let mut model: Option<String> = None;
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;
        let mut cache_read: u64 = 0;
        let mut cache_create: u64 = 0;

        for event in last_per_prompt.values() {
            input_tokens += event.input_tokens;
            output_tokens += event.output_tokens;
            cache_read += event.cache_read_tokens;
            cache_create += event.cache_creation_tokens;
            if model.is_none() {
                model = event.model.clone();
            }
        }

        let duration_ms: u64 = turn_events
            .iter()
            .filter(|t| t.session_id == session_id)
            .map(|t| t.turn_time_ms)
            .sum();

        let duration_secs = if duration_ms > 0 {
            Some(duration_ms / 1000)
        } else {
            None
        };

        Some(NativeStats {
            model,
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            cache_read_tokens: if cache_read > 0 {
                Some(cache_read)
            } else {
                None
            },
            cache_creation_tokens: if cache_create > 0 {
                Some(cache_create)
            } else {
                None
            },
            duration_secs,
            files_touched: Vec::new(),
            tool_call_count: 0,
        })
    }

    /// Return global stats from all telemetry events.
    pub fn global_stats() -> Option<(u64, u64, u64, u64, usize)> {
        if !has_telemetry_log() {
            return None;
        }

        let (usage_events, _) = load_telemetry();
        if usage_events.is_empty() {
            return None;
        }

        // Deduplicate: take last per (thread_id, prompt_id) pair
        let mut last_per_prompt: HashMap<(String, String), &UsageEvent> = HashMap::new();
        for event in &usage_events {
            last_per_prompt.insert((event.thread_id.clone(), event.prompt_id.clone()), event);
        }

        let mut input: u64 = 0;
        let mut output: u64 = 0;
        let mut cache_read: u64 = 0;
        let mut cache_create: u64 = 0;

        for event in last_per_prompt.values() {
            input += event.input_tokens;
            output += event.output_tokens;
            cache_read += event.cache_read_tokens;
            cache_create += event.cache_creation_tokens;
        }

        Some((
            input,
            output,
            cache_read,
            cache_create,
            last_per_prompt.len(),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_strip_provider() {
            assert_eq!(
                strip_provider("zed.dev/claude-sonnet-4-6"),
                "claude-sonnet-4-6"
            );
            assert_eq!(strip_provider("claude-sonnet-4-6"), "claude-sonnet-4-6");
            assert_eq!(strip_provider("openai/gpt-4o"), "openai/gpt-4o");
        }
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
