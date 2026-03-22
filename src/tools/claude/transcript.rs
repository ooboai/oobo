use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::tools::cursor::transcript::Message;

/// Find the transcript file for a Claude session.
pub fn find_transcript_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
    let projects_dir = super::claude_projects_dir()?;
    let slug = super::path_to_slug(project_path);
    let project_dir = projects_dir.join(slug);

    let jsonl = project_dir.join(format!("{session_id}.jsonl"));
    if jsonl.exists() {
        return Some(jsonl);
    }

    if project_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&project_dir) {
            let prefix = &session_id[..session_id.len().min(8)];
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if stem.starts_with(prefix) {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Find subagent transcript files for a given parent Claude session.
/// Returns (subagent_id, file_path) tuples.
pub fn find_subagent_transcripts(project_path: &str, session_id: &str) -> Vec<(String, PathBuf)> {
    let projects_dir = match super::claude_projects_dir() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let slug = super::path_to_slug(project_path);
    let project_dir = projects_dir.join(slug);

    let mut result = Vec::new();

    let session_dir = project_dir.join(session_id);
    let subagents_dir = session_dir.join("subagents");
    if subagents_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&subagents_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        result.push((stem.to_string(), path));
                    }
                }
            }
        }
        return result;
    }

    // Fallback: prefix-match on session_id for directories.
    if project_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&project_dir) {
            let prefix = &session_id[..session_id.len().min(8)];
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with(prefix) {
                    continue;
                }
                let sub_dir = dir.join("subagents");
                if sub_dir.is_dir() {
                    if let Ok(sub_entries) = fs::read_dir(&sub_dir) {
                        for sub_entry in sub_entries.flatten() {
                            let path = sub_entry.path();
                            if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                    result.push((stem.to_string(), path));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

/// Count user/assistant messages in a Claude session file.
pub fn count_messages(project_path: &str, session_id: &str) -> u32 {
    let path = match find_transcript_path(project_path, session_id) {
        Some(p) => p,
        None => return 0,
    };
    count_messages_in_file(&path)
}

fn count_messages_in_file(path: &Path) -> u32 {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };

    let reader = std::io::BufReader::new(file);
    let mut count = 0u32;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
            let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if entry_type == "user" || entry_type == "assistant" {
                count += 1;
            }
        }
    }

    count
}

/// Parse Claude transcript into structured messages.
pub fn parse_messages(path: &Path) -> Vec<Message> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = std::io::BufReader::new(file);
    let mut messages = Vec::new();

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

        match entry_type {
            "user" => {
                if let Some(text) = extract_user_text(&entry) {
                    if !text.is_empty() {
                        messages.push(Message {
                            role: "user".to_string(),
                            text,
                            timestamp_ms: None,
                        });
                    }
                }
            }
            "assistant" => {
                let text = extract_assistant_text(&entry);
                if !text.is_empty() {
                    messages.push(Message {
                        role: "assistant".to_string(),
                        text,
                        timestamp_ms: None,
                    });
                }
            }
            _ => {}
        }
    }

    messages
}

/// Parse Claude JSONL transcript lines into rich structured messages.
/// This is the canonical implementation — used by both file-based parsing
/// and inline string-based parsing in the interceptor.
pub fn parse_rich_transcript_lines<'a>(
    lines: impl Iterator<Item = &'a str>,
) -> Vec<crate::remote::payload::TranscriptMessage> {
    use crate::remote::payload::{ToolCallMessage, ToolResultMessage, TranscriptMessage};
    use crate::utils::{summarize_tool_input, truncate_str};

    let mut messages = Vec::new();
    // Maps tool_use_id → tool name so we can populate ToolResultMessage.name.
    let mut tool_name_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = super::parse_timestamp(&entry);

        match entry_type {
            "user" => {
                // Extract text from user messages.
                if let Some(text) = extract_user_text(&entry) {
                    if !text.is_empty() {
                        messages.push(TranscriptMessage {
                            role: "user".to_string(),
                            text: Some(text),
                            thinking: None,
                            tool_call: None,
                            tool_result: None,
                            timestamp_ms: ts,
                        });
                    }
                }

                // In Claude's JSONL, tool_result blocks appear in user entries.
                if let Some(content) = entry
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for part in content {
                        if part.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                            continue;
                        }
                        let tool_use_id = part
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let is_error = part
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let output =
                            extract_tool_result_output(part).map(|s| truncate_str(&s, 500));
                        let name = tool_name_map.get(&tool_use_id).cloned().unwrap_or_default();

                        messages.push(TranscriptMessage {
                            role: "tool".to_string(),
                            text: None,
                            thinking: None,
                            tool_call: None,
                            tool_result: Some(ToolResultMessage {
                                tool_use_id,
                                name,
                                success: !is_error,
                                output_summary: output,
                            }),
                            timestamp_ms: ts,
                        });
                    }
                }
            }
            "assistant" => {
                if let Some(content) = entry
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    // Accumulate all parts first, then emit in correct order:
                    // thinking → text → tool_calls.
                    let mut text_parts = Vec::new();
                    let mut thinking_parts = Vec::new();
                    let mut tool_calls = Vec::new();

                    for part in content {
                        let pt = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match pt {
                            "text" => {
                                if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                                    text_parts.push(t);
                                }
                            }
                            "thinking" => {
                                if let Some(t) = part.get("thinking").and_then(|v| v.as_str()) {
                                    thinking_parts.push(t);
                                }
                            }
                            "tool_use" => {
                                let tool_use_id = part
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let name = part
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let input_summary =
                                    summarize_tool_input(&name, part.get("input"), 300)
                                        .unwrap_or_default();

                                if !name.is_empty() {
                                    tool_name_map.insert(tool_use_id.clone(), name.clone());
                                    tool_calls.push(TranscriptMessage {
                                        role: "assistant".to_string(),
                                        text: None,
                                        thinking: None,
                                        tool_call: Some(ToolCallMessage {
                                            tool_use_id,
                                            name,
                                            input_summary,
                                        }),
                                        tool_result: None,
                                        timestamp_ms: ts,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }

                    // Emit in correct order: thinking → text → tool_calls.
                    if !thinking_parts.is_empty() {
                        let thinking = thinking_parts.join("\n");
                        messages.push(TranscriptMessage {
                            role: "assistant".to_string(),
                            text: None,
                            thinking: Some(truncate_str(&thinking, 2000)),
                            tool_call: None,
                            tool_result: None,
                            timestamp_ms: ts,
                        });
                    }
                    if !text_parts.is_empty() {
                        let text = text_parts.join("\n").trim_end().to_string();
                        if !text.is_empty() {
                            messages.push(TranscriptMessage {
                                role: "assistant".to_string(),
                                text: Some(text),
                                thinking: None,
                                tool_call: None,
                                tool_result: None,
                                timestamp_ms: ts,
                            });
                        }
                    }
                    messages.extend(tool_calls);
                }
            }
            _ => {}
        }
    }

    messages
}

fn extract_tool_result_output(part: &serde_json::Value) -> Option<String> {
    let content = part.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        let texts: Vec<&str> = arr
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    p.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }
    None
}

/// Read a Claude transcript as formatted text.
pub fn read_transcript(path: &Path, max_messages: u32) -> String {
    let messages = parse_messages(path);
    let mut output = Vec::new();
    let mut count = 0u32;

    for msg in &messages {
        output.push(format!("{}:", msg.role));
        output.push(msg.text.clone());
        output.push(String::new());
        count += 1;
        if count >= max_messages {
            output.push(format!("... (truncated at {max_messages} messages)"));
            break;
        }
    }

    output.join("\n")
}

/// Extract session stats from a Claude transcript file.
pub fn extract_stats(path: &Path) -> Option<crate::remote::payload::SessionStats> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut model: Option<String> = None;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_read_tokens: u64 = 0;
    let mut cache_creation_tokens: u64 = 0;
    let mut files_touched: Vec<String> = Vec::new();
    let mut tool_call_count: u32 = 0;
    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(ts) = super::parse_timestamp(&entry) {
            if first_ts.is_none() {
                first_ts = Some(ts);
            }
            last_ts = Some(ts);
        }

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if entry_type == "assistant" {
            if let Some(msg) = entry.get("message") {
                if model.is_none() {
                    if let Some(m) = msg.get("model").and_then(|v| v.as_str()) {
                        model = Some(m.to_string());
                    }
                }

                if let Some(usage) = msg.get("usage") {
                    input_tokens += usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    output_tokens += usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    cache_read_tokens += usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    cache_creation_tokens += usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }

                if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
                    for part in content {
                        let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if part_type == "tool_use" {
                            tool_call_count += 1;
                            let tool_name = part.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if (tool_name == "Write" || tool_name == "Edit")
                                || tool_name == "MultiEdit"
                            {
                                if let Some(input) = part.get("input") {
                                    if let Some(fp) =
                                        input.get("file_path").and_then(|v| v.as_str())
                                    {
                                        let f = fp.to_string();
                                        if !files_touched.contains(&f) {
                                            files_touched.push(f);
                                        }
                                    }
                                    if let Some(fp) = input.get("path").and_then(|v| v.as_str()) {
                                        let f = fp.to_string();
                                        if !files_touched.contains(&f) {
                                            files_touched.push(f);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if entry_type == "result" {
            if let Some(result) = entry.get("result") {
                if let Some(usage) = result.get("usage") {
                    input_tokens += usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    output_tokens += usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }
            }
        }
    }

    let duration_secs = match (first_ts, last_ts) {
        (Some(f), Some(l)) if l > f => Some(((l - f) / 1000) as u64),
        _ => None,
    };

    Some(crate::remote::payload::SessionStats {
        model,
        input_tokens: if input_tokens > 0 {
            Some(input_tokens)
        } else {
            None
        },
        output_tokens: if output_tokens > 0 {
            Some(output_tokens)
        } else {
            None
        },
        cache_read_tokens: if cache_read_tokens > 0 {
            Some(cache_read_tokens)
        } else {
            None
        },
        cache_creation_tokens: if cache_creation_tokens > 0 {
            Some(cache_creation_tokens)
        } else {
            None
        },
        is_estimated: false,
        token_source: Some("native".to_string()),
        duration_secs,
        files_touched,
        tool_call_count,
    })
}

/// Convenience wrapper for callers that have (project_path, session_id)
/// instead of a direct file path. Kept for API consistency with other
/// tool modules (codex, gemini, copilot, opencode).
#[allow(dead_code)]
pub(crate) fn stats_for_session(
    project_path: &str,
    session_id: &str,
) -> Option<crate::remote::payload::SessionStats> {
    let path = find_transcript_path(project_path, session_id)?;
    extract_stats(&path)
}

/// Extract native telemetry suitable for the analytics pipeline.
pub fn extract_native_stats(
    project_path: &str,
    session_id: &str,
) -> Option<crate::analytics::NativeStats> {
    let path = find_transcript_path(project_path, session_id)?;
    let stats = extract_stats(&path)?;
    Some(crate::analytics::NativeStats {
        model: stats.model,
        input_tokens: stats.input_tokens,
        output_tokens: stats.output_tokens,
        cache_read_tokens: stats.cache_read_tokens,
        cache_creation_tokens: stats.cache_creation_tokens,
        duration_secs: stats.duration_secs,
        files_touched: stats.files_touched,
        tool_call_count: stats.tool_call_count,
    })
}

fn extract_user_text(entry: &serde_json::Value) -> Option<String> {
    let msg = entry.get("message")?;
    let content = msg.get("content")?;

    if let Some(s) = content.as_str() {
        return Some(s.trim_end().to_string());
    }

    if let Some(arr) = content.as_array() {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                    part.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        return Some(parts.join("\n").trim_end().to_string());
    }

    None
}

fn extract_assistant_text(entry: &serde_json::Value) -> String {
    let msg = match entry.get("message") {
        Some(m) => m,
        None => return String::new(),
    };

    let content = match msg.get("content") {
        Some(c) => c,
        None => return String::new(),
    };

    if let Some(arr) = content.as_array() {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                    part.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        return parts.join("\n").trim_end().to_string();
    }

    if let Some(s) = content.as_str() {
        return s.trim_end().to_string();
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_claude_messages() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"file-history-snapshot","messageId":"abc"}}"#).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":"Hello Claude"}},"uuid":"u1","timestamp":"2026-01-12T22:31:39.855Z"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"model":"claude-opus-4-5","type":"message","role":"assistant","content":[{{"type":"thinking","thinking":"let me think..."}},{{"type":"text","text":"Hi there!"}}]}},"uuid":"a1","timestamp":"2026-01-12T22:32:00.000Z"}}"#
        )
        .unwrap();

        let msgs = parse_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello Claude");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "Hi there!");
    }

    #[test]
    fn test_count_claude_messages() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("count.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"file-history-snapshot"}}"#).unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"content":"hi"}}}}"#).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"hello"}}]}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"type":"system","message":{{"content":"sys"}}}}"#).unwrap();

        assert_eq!(count_messages_in_file(&path), 2);
    }

    #[test]
    fn test_assistant_thinking_filtered() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("thinking.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"content":[{{"type":"thinking","thinking":"internal thought"}},{{"type":"text","text":"Visible reply"}}]}}}}"#
        )
        .unwrap();

        let msgs = parse_messages(&path);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "Visible reply");
        assert!(!msgs[0].text.contains("internal thought"));
    }

    #[test]
    fn test_read_transcript_truncation() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trunc.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        for i in 0..10 {
            writeln!(f, r#"{{"type":"user","message":{{"content":"msg {i}"}}}}"#).unwrap();
        }

        let text = read_transcript(&path, 3);
        assert!(text.contains("truncated at 3"));
    }

    #[test]
    fn test_parse_rich_transcript_lines() {
        let jsonl = [
            r#"{"type":"user","message":{"content":"Fix the bug"},"timestamp":"2025-01-15T10:00:00Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Let me analyze the code..."},{"type":"text","text":"I found the issue."},{"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"/src/main.rs"}}]},"timestamp":"2025-01-15T10:00:01Z"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_1","content":"fn main() {}"}]},"timestamp":"2025-01-15T10:00:02Z"}"#,
        ];
        let text = jsonl.join("\n");
        let msgs = parse_rich_transcript_lines(text.lines());

        // user text → thinking → assistant text → tool_call → tool_result
        assert_eq!(msgs.len(), 5);

        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text.as_deref(), Some("Fix the bug"));

        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].thinking.is_some());
        assert!(msgs[1].thinking.as_deref().unwrap().contains("analyze"));

        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[2].text.as_deref(), Some("I found the issue."));

        assert_eq!(msgs[3].role, "assistant");
        let tc = msgs[3].tool_call.as_ref().unwrap();
        assert_eq!(tc.tool_use_id, "tu_1");
        assert_eq!(tc.name, "Read");
        assert_eq!(tc.input_summary, "/src/main.rs");

        assert_eq!(msgs[4].role, "tool");
        let tr = msgs[4].tool_result.as_ref().unwrap();
        assert_eq!(tr.tool_use_id, "tu_1");
        assert_eq!(tr.name, "Read"); // populated via ID→name map
        assert!(tr.success);
        assert_eq!(tr.output_summary.as_deref(), Some("fn main() {}"));
    }

    #[test]
    fn test_rich_transcript_unicode_truncation() {
        let long_thinking = "思考".repeat(1500); // 3000 chars, well over 2000
        let jsonl = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"thinking","thinking":"{long_thinking}"}}]}}}}"#
        );
        let msgs = parse_rich_transcript_lines(jsonl.lines());
        assert_eq!(msgs.len(), 1);
        let thinking = msgs[0].thinking.as_ref().unwrap();
        assert!(thinking.ends_with("..."));
        assert!(thinking.chars().count() <= 2004); // 2000 + "..."
    }

    #[test]
    fn test_rich_transcript_empty_input() {
        let msgs = parse_rich_transcript_lines("".lines());
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_rich_transcript_message_ordering() {
        let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"step1"},{"type":"text","text":"result"},{"type":"tool_use","id":"tu_x","name":"Bash","input":{"command":"ls"}}]}}"#;
        let msgs = parse_rich_transcript_lines(jsonl.lines());

        assert_eq!(msgs.len(), 3);
        // thinking comes first
        assert!(msgs[0].thinking.is_some());
        // then text
        assert!(msgs[1].text.is_some());
        // then tool_call
        assert!(msgs[2].tool_call.is_some());
    }
}
