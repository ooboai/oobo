/// Junie CLI (JetBrains) — session storage in ~/.junie/.
///
/// Note: Junie is in beta (as of Q1 2026). Storage format may change.
/// This implementation scans for JSON/JSONL session files in the Junie data dir.
/// Junie imports from .claude/ hooks, so Claude hooks may already capture some activity.
use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::cursor::Session;

pub fn junie_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".junie"))
}

pub fn sessions_dir() -> Option<PathBuf> {
    // Junie may store sessions in ~/.junie/sessions/ or ~/.local/share/junie/
    if let Some(home) = dirs::home_dir() {
        let candidates = [
            home.join(".junie/sessions"),
            home.join(".local/share/junie/sessions"),
        ];
        for c in candidates {
            if c.exists() {
                return Some(c);
            }
        }
    }
    // Fall back to ~/.junie itself
    junie_dir()
}

fn session_from_file(path: &Path) -> Option<Session> {
    let session_id = path.file_stem().and_then(|s| s.to_str())?.to_string();
    let content = fs::read_to_string(path).ok()?;

    // Try JSON first
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
        let name = data
            .get("title")
            .or_else(|| data.get("name"))
            .or_else(|| data.get("summary"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty()).map_or_else(|| "Junie session".to_string(), |s| crate::utils::truncate_name(s, crate::utils::MAX_SESSION_NAME_LEN));

        let project_path = data
            .get("cwd")
            .or_else(|| data.get("workingDirectory"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mtime = fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64
            });

        return Some(Session {
            session_id,
            name,
            mode: "junie".to_string(),
            created_at: mtime,
            updated_at: mtime,
            project_path,
            workspace_dir: path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            source: "junie".to_string(),
            parent_session_id: None,
            subagent_type: None,
        });
    }

    // Try as JSONL — scan lines for cwd and title
    let mut jsonl_name: Option<String> = None;
    let mut jsonl_project_path = String::new();
    let mut has_any_json_line = false;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let data: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        has_any_json_line = true;

        if jsonl_name.is_none() {
            if let Some(title) = data
                .get("title")
                .or_else(|| data.get("summary"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                jsonl_name = Some(crate::utils::truncate_name(
                    title,
                    crate::utils::MAX_SESSION_NAME_LEN,
                ));
            }
        }

        if jsonl_project_path.is_empty() {
            if let Some(cwd) = data
                .get("cwd")
                .or_else(|| data.get("workingDirectory"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                jsonl_project_path = cwd.to_string();
            }
        }

        if jsonl_name.is_some() && !jsonl_project_path.is_empty() {
            break;
        }
    }

    if has_any_json_line {
        let mtime = fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64
            });

        return Some(Session {
            session_id,
            name: jsonl_name.unwrap_or_else(|| "Junie session".to_string()),
            mode: "junie".to_string(),
            created_at: mtime,
            updated_at: mtime,
            project_path: jsonl_project_path,
            workspace_dir: path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            source: "junie".to_string(),
            parent_session_id: None,
            subagent_type: None,
        });
    }

    None
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let dir = match sessions_dir() {
        Some(d) if d.exists() => d,
        _ => return Ok(Vec::new()),
    };

    let mut sessions = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                let ext = p.extension().and_then(|e| e.to_str());
                if matches!(ext, Some("json" | "jsonl")) {
                    if let Some(s) = session_from_file(&p) {
                        sessions.push(s);
                    }
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
        let dir = super::sessions_dir()?;
        for ext in ["json", "jsonl"] {
            let p = dir.join(format!("{session_id}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        // Try single JSON object with "messages" array
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(msgs) = data.get("messages").and_then(|m| m.as_array()) {
                return extract_messages_from_array(msgs);
            }
        }

        // Fall back to JSONL: each line is a message
        let mut messages = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let role = entry.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role != "user" && role != "assistant" {
                continue;
            }
            let text = entry
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if !text.is_empty() {
                let ts = entry.get("timestamp").and_then(serde_json::Value::as_i64);
                messages.push(Message {
                    role: role.to_string(),
                    text,
                    timestamp_ms: ts,
                });
            }
        }
        messages
    }

    fn extract_messages_from_array(arr: &[serde_json::Value]) -> Vec<Message> {
        let mut messages = Vec::new();
        for msg in arr {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role != "user" && role != "assistant" {
                continue;
            }
            let text = msg
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if !text.is_empty() {
                let ts = msg.get("timestamp").and_then(serde_json::Value::as_i64);
                messages.push(Message {
                    role: role.to_string(),
                    text,
                    timestamp_ms: ts,
                });
            }
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_from_json_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-abc.json");
        fs::write(
            &path,
            r#"{"title":"Fix database migration","cwd":"/dev/myapp","messages":[]}"#,
        )
        .unwrap();

        let session = session_from_file(&path).unwrap();
        assert_eq!(session.session_id, "sess-abc");
        assert_eq!(session.name, "Fix database migration");
        assert_eq!(session.project_path, "/dev/myapp");
        assert_eq!(session.source, "junie");
    }

    #[test]
    fn test_session_from_jsonl_with_cwd() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-def.jsonl");
        fs::write(
            &path,
            // First line has no cwd, second line does
            r#"{"title":"Refactor API"}
{"role":"user","cwd":"/dev/api-server","content":"Help refactor"}
"#,
        )
        .unwrap();

        let session = session_from_file(&path).unwrap();
        assert_eq!(session.name, "Refactor API");
        assert_eq!(session.project_path, "/dev/api-server");
    }

    #[test]
    fn test_session_from_jsonl_empty_project_path_no_cwd() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-ghi.jsonl");
        fs::write(
            &path,
            r#"{"role":"user","content":"Hello"}
"#,
        )
        .unwrap();

        let session = session_from_file(&path).unwrap();
        assert_eq!(session.name, "Junie session");
        assert!(session.project_path.is_empty());
    }

    #[test]
    fn test_session_from_multiline_jsonl_no_metadata() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-multi.jsonl");
        fs::write(
            &path,
            r#"{"role":"user","content":"Hello"}
{"role":"assistant","content":"Hi there!"}
"#,
        )
        .unwrap();

        // Multi-line JSONL with no title/cwd should still return a session
        let session = session_from_file(&path).unwrap();
        assert_eq!(session.name, "Junie session");
        assert!(session.project_path.is_empty());
        assert_eq!(session.source, "junie");
    }

    #[test]
    fn test_all_sessions_empty_dir() {
        let result = all_sessions();
        assert!(result.is_ok());
    }

    #[test]
    fn test_session_from_binary_garbage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("garbage.json");
        fs::write(&path, [0xFF, 0xFE, 0x00, 0x01, 0x80]).unwrap();
        assert!(session_from_file(&path).is_none());
    }

    #[test]
    fn test_session_from_empty_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("empty.jsonl");
        fs::write(&path, "").unwrap();
        assert!(session_from_file(&path).is_none());
    }

    #[test]
    fn test_transcript_parse_json_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess.json");
        fs::write(
            &path,
            r#"{"title":"Test","messages":[{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi!"},{"role":"system","content":"ignored"}]}"#,
        )
        .unwrap();

        let msgs = transcript::parse_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "Hi!");
    }

    #[test]
    fn test_transcript_parse_jsonl_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess.jsonl");
        fs::write(
            &path,
            r#"{"role":"user","content":"What is Rust?","timestamp":1000}
{"role":"assistant","content":"A systems language.","timestamp":2000}
{"role":"system","content":"ignored"}
"#,
        )
        .unwrap();

        let msgs = transcript::parse_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "What is Rust?");
        assert_eq!(msgs[0].timestamp_ms, Some(1000));
        assert_eq!(msgs[1].text, "A systems language.");
    }

    #[test]
    fn test_transcript_parse_empty_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("empty.json");
        fs::write(&path, "").unwrap();
        let msgs = transcript::parse_messages(&path);
        assert!(msgs.is_empty());
    }
}
