use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::cursor::transcript::Message;
use crate::tools::cursor::Session;

fn gemini_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".gemini"))
}

fn tmp_dir() -> Option<PathBuf> {
    gemini_dir().map(|d| d.join("tmp"))
}

/// Load `~/.gemini/projects.json` which maps absolute paths to project slugs.
fn load_projects_map() -> HashMap<String, String> {
    let path = match gemini_dir() {
        Some(d) => d.join("projects.json"),
        None => return HashMap::new(),
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    let obj = match v.get("projects").and_then(|p| p.as_object()) {
        Some(o) => o,
        None => return HashMap::new(),
    };
    let mut map = HashMap::new();
    for (abs_path, slug) in obj {
        if let Some(s) = slug.as_str() {
            map.insert(abs_path.clone(), s.to_string());
        }
    }
    map
}

/// Build the reverse map: project_slug → absolute_path.
fn slug_to_path_map() -> HashMap<String, String> {
    load_projects_map()
        .into_iter()
        .map(|(path, slug)| (slug, path))
        .collect()
}

/// List all session JSON files across all project directories under ~/.gemini/tmp/.
fn find_all_session_files() -> Vec<(PathBuf, String)> {
    let tmp = match tmp_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Vec::new(),
    };

    let slug_map = slug_to_path_map();
    let mut results = Vec::new();

    let entries = match fs::read_dir(&tmp) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let slug = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let project_path = slug_map.get(&slug).cloned().unwrap_or_default();

        let chats_dir = project_dir.join("chats");
        if !chats_dir.is_dir() {
            continue;
        }

        let files = match fs::read_dir(&chats_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for file_entry in files.flatten() {
            let p = file_entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("session-") {
                    results.push((p, project_path.clone()));
                }
            }
        }
    }

    results
}

/// Parse a Gemini CLI session JSON file into a Session.
fn session_from_file(path: &Path, project_path: &str) -> Option<Session> {
    let content = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;

    let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    let is_subagent = kind == "subagent";

    let session_id = v.get("sessionId")?.as_str()?.to_string();
    let start_time = v.get("startTime").and_then(|s| s.as_str()).unwrap_or("");
    let last_updated = v.get("lastUpdated").and_then(|s| s.as_str()).unwrap_or("");

    let messages = v.get("messages").and_then(|m| m.as_array())?;

    let has_content = messages.iter().any(|m| {
        matches!(
            m.get("type").and_then(|t| t.as_str()),
            Some("user" | "gemini")
        )
    });
    if !has_content {
        return None;
    }

    let name = v
        .get("summary")
        .and_then(|s| s.as_str())
        .map(|s| crate::utils::truncate_name(s, crate::utils::MAX_SESSION_NAME_LEN))
        .unwrap_or_else(|| extract_first_user_message(messages));

    let created_at = crate::utils::parse_iso_timestamp(start_time);
    let updated_at = crate::utils::parse_iso_timestamp(last_updated);

    let (parent_session_id, subagent_type) = if is_subagent {
        let parent = v
            .get("parentSessionId")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let agent_name = v
            .get("agentName")
            .or_else(|| v.get("agentId"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some("unknown".to_string()));
        (parent, agent_name)
    } else {
        (None, None)
    };

    Some(Session {
        session_id,
        name,
        mode: "gemini".to_string(),
        created_at,
        updated_at,
        project_path: project_path.to_string(),
        workspace_dir: String::new(),
        source: "gemini".to_string(),
        parent_session_id,
        subagent_type,
    })
}

fn extract_first_user_message(messages: &[serde_json::Value]) -> String {
    for msg in messages {
        if msg.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let text = content_to_string(msg.get("content"));
        let trimmed = text.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('/') && !trimmed.starts_with('?') {
            return crate::utils::truncate_name(trimmed, crate::utils::MAX_SESSION_NAME_LEN);
        }
    }
    // Fallback: take any user message even if it's a command
    for msg in messages {
        if msg.get("type").and_then(|t| t.as_str()) == Some("user") {
            let text = content_to_string(msg.get("content"));
            if !text.trim().is_empty() {
                return crate::utils::truncate_name(
                    text.trim(),
                    crate::utils::MAX_SESSION_NAME_LEN,
                );
            }
        }
    }
    "Gemini session".to_string()
}

/// Convert Gemini's content field (string or Part array) to a plain string.
fn content_to_string(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => {
            let mut texts = Vec::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    texts.push(text);
                }
            }
            texts.join("\n")
        }
        _ => String::new(),
    }
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let norm = crate::paths::normalize_path(project_root);
    let mut sessions = all_sessions()?;
    sessions.retain(|s| {
        !s.project_path.is_empty() && crate::paths::normalize_path(&s.project_path) == norm
    });
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let files = find_all_session_files();
    let mut sessions: Vec<Session> = files
        .iter()
        .filter_map(|(path, project_path)| session_from_file(path, project_path))
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use super::*;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        let tmp = tmp_dir()?;
        if !tmp.is_dir() {
            return None;
        }
        let entries = fs::read_dir(&tmp).ok()?;
        for entry in entries.flatten() {
            let chats_dir = entry.path().join("chats");
            if !chats_dir.is_dir() {
                continue;
            }
            let files = match fs::read_dir(&chats_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for file_entry in files.flatten() {
                let p = file_entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with("session-") {
                    continue;
                }
                // Match by UUID prefix in filename or by reading sessionId
                let short_id = &session_id[..session_id.len().min(8)];
                if name.contains(short_id) {
                    return Some(p);
                }
            }
        }
        // Fallback: read files to match by sessionId field
        let entries = fs::read_dir(&tmp).ok()?;
        for entry in entries.flatten() {
            let chats_dir = entry.path().join("chats");
            if !chats_dir.is_dir() {
                continue;
            }
            let files = match fs::read_dir(&chats_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for file_entry in files.flatten() {
                let p = file_entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&p) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                        if v.get("sessionId").and_then(|s| s.as_str()) == Some(session_id) {
                            return Some(p);
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
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let v: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let messages = match v.get("messages").and_then(|m| m.as_array()) {
            Some(m) => m,
            None => return Vec::new(),
        };

        let mut result = Vec::new();
        for msg in messages {
            let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let role = match msg_type {
                "user" => "user",
                "gemini" => "assistant",
                _ => continue,
            };

            let text = content_to_string(msg.get("content"));
            let display = content_to_string(msg.get("displayContent"));
            let final_text = if !display.is_empty() { display } else { text };

            if final_text.trim().is_empty() {
                continue;
            }

            let ts = msg
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(crate::utils::parse_iso_timestamp);

            result.push(Message {
                role: role.to_string(),
                text: final_text,
                timestamp_ms: ts,
            });
        }

        result
    }

    /// Extract native token stats from a Gemini CLI session file.
    pub fn extract_stats(path: &Path) -> Option<crate::remote::payload::SessionStats> {
        let content = fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&content).ok()?;

        let start_time = v.get("startTime").and_then(|s| s.as_str()).unwrap_or("");
        let last_updated = v.get("lastUpdated").and_then(|s| s.as_str()).unwrap_or("");

        let messages = v.get("messages").and_then(|m| m.as_array())?;

        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;
        let mut total_cached: u64 = 0;
        let mut total_thoughts: u64 = 0;
        let mut tool_call_count: u32 = 0;
        let mut files_touched: Vec<String> = Vec::new();
        let mut model: Option<String> = None;

        for msg in messages {
            let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if msg_type != "gemini" {
                continue;
            }

            if model.is_none() {
                model = msg
                    .get("model")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string());
            }

            if let Some(tokens) = msg.get("tokens") {
                total_input += tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
                total_output += tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
                total_cached += tokens.get("cached").and_then(|v| v.as_u64()).unwrap_or(0);
                total_thoughts += tokens.get("thoughts").and_then(|v| v.as_u64()).unwrap_or(0);
            }

            if let Some(tool_calls) = msg.get("toolCalls").and_then(|tc| tc.as_array()) {
                tool_call_count += tool_calls.len() as u32;
                for tc in tool_calls {
                    extract_files_from_tool_call(tc, &mut files_touched);
                }
            }
        }

        total_output += total_thoughts;

        let duration_secs = match (
            crate::utils::parse_iso_timestamp(start_time),
            crate::utils::parse_iso_timestamp(last_updated),
        ) {
            (Some(start), Some(end)) if end > start => Some(((end - start) / 1000) as u64),
            _ => None,
        };

        let has_tokens = total_input > 0 || total_output > 0;

        Some(crate::remote::payload::SessionStats {
            model,
            input_tokens: if has_tokens { Some(total_input) } else { None },
            output_tokens: if has_tokens { Some(total_output) } else { None },
            cache_read_tokens: if total_cached > 0 {
                Some(total_cached)
            } else {
                None
            },
            cache_creation_tokens: None,
            is_estimated: false,
            token_source: if has_tokens {
                Some("native".to_string())
            } else {
                None
            },
            duration_secs,
            files_touched,
            tool_call_count,
        })
    }

    fn extract_files_from_tool_call(tc: &serde_json::Value, files: &mut Vec<String>) {
        let name = tc.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let display_name = tc.get("displayName").and_then(|n| n.as_str()).unwrap_or("");

        let file_tools = [
            "readFile",
            "writeFile",
            "editFile",
            "read_file",
            "write_file",
            "edit_file",
            "ReadFile",
            "WriteFile",
            "EditFile",
        ];

        let is_file_tool = file_tools
            .iter()
            .any(|t| name.contains(t) || display_name.contains(t))
            || name.contains("file")
            || display_name.contains("file");

        if !is_file_tool {
            return;
        }

        // Gemini CLI uses "args" (real data) or "input" (older format)
        let candidates = [tc.get("args"), tc.get("input")];
        for maybe_obj in candidates.into_iter().flatten() {
            for key in ["path", "file_path", "filePath", "file"] {
                if let Some(fp) = maybe_obj.get(key).and_then(|v| v.as_str()) {
                    let f = fp.to_string();
                    if !files.contains(&f) {
                        files.push(f);
                    }
                }
            }
            if let Some(input_str) = maybe_obj.as_str() {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(input_str) {
                    for key in ["path", "file_path", "filePath", "file"] {
                        if let Some(fp) = parsed.get(key).and_then(|v| v.as_str()) {
                            let f = fp.to_string();
                            if !files.contains(&f) {
                                files.push(f);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn stats_for_session(
        _project_path: &str,
        session_id: &str,
    ) -> Option<crate::remote::payload::SessionStats> {
        let path = find_transcript_path("", session_id)?;
        extract_stats(&path)
    }

    pub fn read_transcript(path: &Path, max_messages: u32) -> String {
        let messages = parse_messages(path);
        crate::utils::format_transcript(&messages, max_messages, "Gemini")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gemini_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_file = tmp.path().join("session-2026-03-05T10-00-abcd1234.json");
        fs::write(
            &session_file,
            r#"{
  "sessionId": "abcd1234-5678-90ab-cdef-1234567890ab",
  "projectHash": "test-project",
  "startTime": "2026-03-05T10:00:00.000Z",
  "lastUpdated": "2026-03-05T10:05:30.000Z",
  "messages": [
    {
      "id": "msg-1",
      "timestamp": "2026-03-05T10:00:01.000Z",
      "type": "user",
      "content": "Fix the authentication bug in login.ts"
    },
    {
      "id": "msg-2",
      "timestamp": "2026-03-05T10:00:10.000Z",
      "type": "gemini",
      "content": "I'll investigate the auth issue in login.ts...",
      "model": "gemini-2.5-pro",
      "tokens": {
        "input": 1500,
        "output": 800,
        "cached": 200,
        "thoughts": 150,
        "tool": 100,
        "total": 2750
      },
      "toolCalls": [
        {
          "id": "tc-1",
          "name": "readFile",
          "input": {"path": "src/login.ts"}
        }
      ]
    },
    {
      "id": "msg-3",
      "timestamp": "2026-03-05T10:01:00.000Z",
      "type": "user",
      "content": "Looks good, apply the fix"
    },
    {
      "id": "msg-4",
      "timestamp": "2026-03-05T10:01:30.000Z",
      "type": "gemini",
      "content": "Done! The authentication bug has been fixed.",
      "model": "gemini-2.5-pro",
      "tokens": {
        "input": 2000,
        "output": 500,
        "cached": 300,
        "thoughts": 50,
        "tool": 200,
        "total": 3050
      },
      "toolCalls": [
        {
          "id": "tc-2",
          "name": "editFile",
          "input": {"path": "src/login.ts"}
        },
        {
          "id": "tc-3",
          "name": "writeFile",
          "input": {"path": "src/auth/utils.ts"}
        }
      ]
    }
  ]
}"#,
        )
        .unwrap();

        let session = session_from_file(&session_file, "/home/dev/project").unwrap();
        assert_eq!(session.source, "gemini");
        assert_eq!(session.name, "Fix the authentication bug in login.ts");
        assert_eq!(session.project_path, "/home/dev/project");
        assert_eq!(session.session_id, "abcd1234-5678-90ab-cdef-1234567890ab");

        let msgs = transcript::parse_messages(&session_file);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Fix the authentication bug in login.ts");
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].text.contains("auth issue"));

        let stats = transcript::extract_stats(&session_file).unwrap();
        assert_eq!(stats.model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(stats.input_tokens, Some(3500)); // 1500 + 2000
        assert_eq!(stats.output_tokens, Some(1500)); // 800+150 + 500+50
        assert_eq!(stats.cache_read_tokens, Some(500)); // 200 + 300
        assert_eq!(stats.tool_call_count, 3);
        assert!(stats.files_touched.contains(&"src/login.ts".to_string()));
        assert!(stats
            .files_touched
            .contains(&"src/auth/utils.ts".to_string()));
        assert!(!stats.is_estimated);
        assert_eq!(stats.token_source.as_deref(), Some("native"));
    }

    #[test]
    fn test_parse_iso_timestamp() {
        let ts = crate::utils::parse_iso_timestamp("2026-03-05T10:00:00.000Z");
        assert!(ts.is_some());
        assert!(ts.unwrap() > 0);
    }

    #[test]
    fn test_subagent_sessions_included_with_metadata() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_file = tmp.path().join("session-sub.json");
        fs::write(
            &session_file,
            r#"{
  "sessionId": "sub-1234",
  "projectHash": "test",
  "startTime": "2026-03-05T10:00:00.000Z",
  "lastUpdated": "2026-03-05T10:00:30.000Z",
  "kind": "subagent",
  "parentSessionId": "parent-5678",
  "agentName": "code_reviewer",
  "messages": [
    {"id": "m1", "timestamp": "2026-03-05T10:00:01.000Z", "type": "user", "content": "do something"}
  ]
}"#,
        )
        .unwrap();

        let session = session_from_file(&session_file, "/tmp").unwrap();
        assert_eq!(session.session_id, "sub-1234");
        assert_eq!(
            session.parent_session_id.as_deref(),
            Some("parent-5678")
        );
        assert_eq!(session.subagent_type.as_deref(), Some("code_reviewer"));
        assert!(session.is_subagent());
    }

    #[test]
    fn test_subagent_without_parent_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_file = tmp.path().join("session-sub-orphan.json");
        fs::write(
            &session_file,
            r#"{
  "sessionId": "sub-orphan",
  "projectHash": "test",
  "startTime": "2026-03-05T10:00:00.000Z",
  "lastUpdated": "2026-03-05T10:00:30.000Z",
  "kind": "subagent",
  "messages": [
    {"id": "m1", "timestamp": "2026-03-05T10:00:01.000Z", "type": "user", "content": "task"}
  ]
}"#,
        )
        .unwrap();

        let session = session_from_file(&session_file, "/tmp").unwrap();
        assert!(session.parent_session_id.is_none());
        assert_eq!(session.subagent_type.as_deref(), Some("unknown"));
    }

    #[test]
    fn test_empty_session_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_file = tmp.path().join("session-empty.json");
        fs::write(
            &session_file,
            r#"{
  "sessionId": "empty-1234",
  "projectHash": "test",
  "startTime": "2026-03-05T10:00:00.000Z",
  "lastUpdated": "2026-03-05T10:00:30.000Z",
  "messages": [
    {"id": "m1", "timestamp": "2026-03-05T10:00:01.000Z", "type": "info", "content": "Session started"}
  ]
}"#,
        )
        .unwrap();

        assert!(session_from_file(&session_file, "/tmp").is_none());
    }

    #[test]
    fn test_content_as_parts_array() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_file = tmp.path().join("session-parts.json");
        fs::write(
            &session_file,
            r#"{
  "sessionId": "parts-1234",
  "projectHash": "test",
  "startTime": "2026-03-05T10:00:00.000Z",
  "lastUpdated": "2026-03-05T10:00:30.000Z",
  "messages": [
    {
      "id": "m1",
      "timestamp": "2026-03-05T10:00:01.000Z",
      "type": "user",
      "content": [{"text": "Hello, "}, {"text": "fix this bug"}]
    },
    {
      "id": "m2",
      "timestamp": "2026-03-05T10:00:05.000Z",
      "type": "gemini",
      "content": [{"text": "On it!"}],
      "tokens": {"input": 100, "output": 50, "cached": 0, "thoughts": 0, "tool": 0, "total": 150}
    }
  ]
}"#,
        )
        .unwrap();

        let session = session_from_file(&session_file, "/tmp").unwrap();
        assert_eq!(session.name, "Hello, \nfix this bug");

        let msgs = transcript::parse_messages(&session_file);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "Hello, \nfix this bug");
    }
}
