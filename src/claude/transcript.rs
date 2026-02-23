use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::cursor::transcript::Message;

/// Find the transcript file for a Claude session.
pub fn find_transcript_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
    let projects_dir = super::claude_projects_dir()?;
    let slug = super::path_to_slug(project_path);
    let project_dir = projects_dir.join(slug);

    let jsonl = project_dir.join(format!("{session_id}.jsonl"));
    if jsonl.exists() {
        return Some(jsonl);
    }

    // Prefix match
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
                    });
                }
            }
            _ => {}
        }
    }

    messages
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
pub fn extract_stats(path: &Path) -> Option<crate::server::payload::SessionStats> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut model: Option<String> = None;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut total_cost: f64 = 0.0;
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
            if let Some(c) = entry.get("costUSD").and_then(|v| v.as_f64()) {
                total_cost += c;
            }
        }
    }

    let duration_secs = match (first_ts, last_ts) {
        (Some(f), Some(l)) if l > f => Some(((l - f) / 1000) as u64),
        _ => None,
    };

    Some(crate::server::payload::SessionStats {
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
        total_cost_usd: if total_cost > 0.0 {
            Some(total_cost)
        } else {
            None
        },
        duration_secs,
        files_touched,
        tool_call_count,
    })
}

pub fn stats_for_session(
    project_path: &str,
    session_id: &str,
) -> Option<crate::server::payload::SessionStats> {
    let path = find_transcript_path(project_path, session_id)?;
    extract_stats(&path)
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
}
