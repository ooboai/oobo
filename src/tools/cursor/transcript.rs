use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use super::{cursor_projects_dir, path_to_slug};

pub use crate::core::message::Message;

/// Find the transcript file for a session.
pub fn find_transcript_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
    let projects_dir = cursor_projects_dir()?;
    let slug = path_to_slug(project_path);
    let transcripts_dir = projects_dir.join(slug).join("agent-transcripts");

    let subdir = transcripts_dir.join(session_id);
    if subdir.is_dir() {
        let jsonl = subdir.join(format!("{session_id}.jsonl"));
        if jsonl.exists() {
            return Some(jsonl);
        }
    }

    let txt = transcripts_dir.join(format!("{session_id}.txt"));
    if txt.exists() {
        return Some(txt);
    }

    if transcripts_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&transcripts_dir) {
            let prefix = &session_id[..session_id.len().min(8)];
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if entry.path().is_dir() && name_str.starts_with(prefix) {
                    let jsonl = entry.path().join(format!("{name_str}.jsonl"));
                    if jsonl.exists() {
                        return Some(jsonl);
                    }
                } else if entry.path().is_file() {
                    let stem = Path::new(&*name_str)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if stem.starts_with(prefix) {
                        return Some(entry.path());
                    }
                }
            }
        }
    }

    None
}

/// Count messages in a transcript file.
pub fn count_messages(project_path: &str, session_id: &str) -> u32 {
    let path = match find_transcript_path(project_path, session_id) {
        Some(p) => p,
        None => return 0,
    };

    count_messages_in_file(&path)
}

pub fn count_messages_in_file(path: &Path) -> u32 {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };

    let reader = std::io::BufReader::new(file);
    let mut count = 0u32;

    if is_jsonl(path) {
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(role) = entry.get("role").and_then(|v| v.as_str()) {
                    if role == "user" || role == "assistant" {
                        count += 1;
                    }
                }
            }
        }
    } else {
        for line in reader.lines().map_while(Result::ok) {
            if line.starts_with("user:") || line.starts_with("assistant:") {
                count += 1;
            }
        }
    }

    count
}

/// Read a transcript as formatted text.
pub fn read_transcript(path: &Path, max_messages: u32) -> String {
    let messages = parse_messages(path);
    crate::utils::format_transcript(&messages, max_messages, "Assistant")
}

/// Parse transcript into structured messages.
pub fn parse_messages(path: &Path) -> Vec<Message> {
    if is_jsonl(path) {
        parse_jsonl_messages(path)
    } else {
        parse_txt_messages(path)
    }
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
}

#[cfg(test)]
fn read_jsonl_transcript(path: &Path, max_messages: u32) -> String {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return format!("(error reading transcript: {e})"),
    };

    let reader = std::io::BufReader::new(file);
    let mut output = Vec::new();
    let mut count = 0u32;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let role = entry.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        let text = extract_text_from_message(&entry);

        if !text.is_empty() {
            output.push(format!("{role}:"));
            output.push(text);
            output.push(String::new());
            count += 1;
            if count >= max_messages {
                output.push(format!("... (truncated at {max_messages} messages)"));
                break;
            }
        }
    }

    output.join("\n")
}

fn parse_jsonl_messages(path: &Path) -> Vec<Message> {
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

        let role = entry
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let text = extract_text_from_message(&entry);

        if !role.is_empty() && !text.is_empty() {
            messages.push(Message {
                role,
                text,
                timestamp_ms: None,
            });
        }
    }

    messages
}

fn parse_txt_messages(path: &Path) -> Vec<Message> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut messages = Vec::new();
    let mut current_role = String::new();
    let mut current_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        if line.starts_with("user:") || line.starts_with("assistant:") {
            if !current_role.is_empty() && !current_lines.is_empty() {
                messages.push(Message {
                    role: current_role.clone(),
                    text: current_lines.join("\n"),
                    timestamp_ms: None,
                });
            }
            let colon = match line.find(':') {
                Some(pos) => pos,
                None => continue,
            };
            current_role = line[..colon].to_string();
            let rest = line[colon + 1..].trim();
            current_lines = if rest.is_empty() {
                Vec::new()
            } else {
                vec![rest.to_string()]
            };
        } else {
            current_lines.push(line.to_string());
        }
    }

    if !current_role.is_empty() && !current_lines.is_empty() {
        messages.push(Message {
            role: current_role,
            text: current_lines.join("\n"),
            timestamp_ms: None,
        });
    }

    messages
}

fn extract_text_from_message(entry: &serde_json::Value) -> String {
    let msg = match entry.get("message") {
        Some(m) => m,
        None => return String::new(),
    };

    let content = match msg.get("content") {
        Some(c) => c,
        None => return String::new(),
    };

    if let Some(s) = content.as_str() {
        return s.trim_end().to_string();
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
        return parts.join("\n").trim_end().to_string();
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_jsonl_messages() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"Hello"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"role":"assistant","message":{{"content":[{{"type":"text","text":"Hi there"}}]}}}}"#
        )
        .unwrap();

        let msgs = parse_jsonl_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "Hi there");
    }

    #[test]
    fn test_parse_txt_messages() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        fs::write(
            &path,
            "user:\nWhat is 2+2?\nassistant:\nThe answer is 4.\nHope that helps!\n",
        )
        .unwrap();

        let msgs = parse_txt_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert!(msgs[0].text.contains("2+2"));
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].text.contains("answer is 4"));
    }

    #[test]
    fn test_count_jsonl() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("count.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"role":"user","message":{{"content":"hi"}}}}"#).unwrap();
        writeln!(
            f,
            r#"{{"role":"assistant","message":{{"content":"hello"}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"role":"system","message":{{"content":"sys"}}}}"#).unwrap();

        assert_eq!(count_messages_in_file(&path), 2); // system excluded
    }

    #[test]
    fn test_count_txt() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("count.txt");
        fs::write(&path, "user:\nhello\nassistant:\nworld\nuser:\nagain\n").unwrap();

        assert_eq!(count_messages_in_file(&path), 3);
    }

    #[test]
    fn test_read_jsonl_transcript() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("read.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"Q1"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"role":"assistant","message":{{"content":[{{"type":"text","text":"A1"}}]}}}}"#
        )
        .unwrap();

        let text = read_jsonl_transcript(&path, 100);
        assert!(text.contains("user:"));
        assert!(text.contains("Q1"));
        assert!(text.contains("assistant:"));
        assert!(text.contains("A1"));
    }

    #[test]
    fn test_read_jsonl_truncation() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trunc.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        for i in 0..10 {
            writeln!(
                f,
                r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"msg {i}"}}]}}}}"#
            )
            .unwrap();
        }

        let text = read_jsonl_transcript(&path, 3);
        assert!(text.contains("truncated at 3"));
    }
}
