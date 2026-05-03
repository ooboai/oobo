/// Continue (continue.dev) — Claude Code-compatible hook format,
/// JSONL sessions in ~/.continue/sessions/<session-id>.jsonl.
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::tools::cursor::Session;

pub fn continue_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".continue"))
}

pub fn sessions_dir() -> Option<PathBuf> {
    continue_dir().map(|d| d.join("sessions"))
}

fn session_from_jsonl(path: &Path) -> Option<Session> {
    let session_id = path.file_stem().and_then(|s| s.to_str())?.to_string();

    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut name: Option<String> = None;
    let mut project_path = String::new();
    let mut created_at: Option<i64> = None;
    let mut updated_at: Option<i64> = None;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Extract title from summary messages
        if name.is_none() {
            if let Some(title) = entry
                .get("summary")
                .or_else(|| entry.get("title"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                name = Some(crate::utils::truncate_name(
                    title,
                    crate::utils::MAX_SESSION_NAME_LEN,
                ));
            }
        }

        // Extract project path from cwd field
        if project_path.is_empty() {
            if let Some(cwd) = entry.get("cwd").and_then(|v| v.as_str()) {
                project_path = cwd.to_string();
            }
        }

        // Track timestamps
        if let Some(ts) = entry.get("timestamp").and_then(serde_json::Value::as_i64) {
            if created_at.is_none() {
                created_at = Some(ts);
            }
            updated_at = Some(ts);
        }
    }

    // Fall back to file mtime for timestamps
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
        name: name.unwrap_or_else(|| "Continue session".to_string()),
        mode: "continue".to_string(),
        created_at: created_at.or(mtime),
        updated_at: updated_at.or(mtime),
        project_path,
        workspace_dir: path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        source: "continue".to_string(),
        parent_session_id: None,
        subagent_type: None,
    })
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let dir = match sessions_dir() {
        Some(d) if d.exists() => d,
        _ => return Ok(Vec::new()),
    };

    let norm_root = crate::paths::normalize_path(project_root);
    let mut sessions = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|e| e == "jsonl") {
                if let Some(s) = session_from_jsonl(&p) {
                    if !s.project_path.is_empty()
                        && crate::paths::normalize_path(&s.project_path) == norm_root
                    {
                        sessions.push(s);
                    }
                }
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
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
            if p.is_file() && p.extension().is_some_and(|e| e == "jsonl") {
                if let Some(s) = session_from_jsonl(&p) {
                    sessions.push(s);
                }
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use std::fs;
    use std::io::BufRead;
    use std::path::{Path, PathBuf};

    use crate::core::message::Message;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        let dir = super::sessions_dir()?;
        let p = dir.join(format!("{session_id}.jsonl"));
        if p.exists() {
            return Some(p);
        }
        None
    }

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
            let role = entry.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role != "user" && role != "assistant" {
                continue;
            }
            let text = entry
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !text.is_empty() {
                messages.push(Message {
                    role: role.to_string(),
                    text,
                    timestamp_ms: entry.get("timestamp").and_then(serde_json::Value::as_i64),
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
    fn test_session_from_jsonl() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-abc.jsonl");
        fs::write(
            &path,
            r#"{"cwd":"/dev/myapp","summary":"Fix build errors"}
{"role":"user","content":"Can you fix the build?","timestamp":1700000000000}
{"role":"assistant","content":"Sure, I see the issue.","timestamp":1700000010000}
"#,
        )
        .unwrap();

        let session = session_from_jsonl(&path).unwrap();
        assert_eq!(session.session_id, "sess-abc");
        assert_eq!(session.name, "Fix build errors");
        assert_eq!(session.project_path, "/dev/myapp");
        assert_eq!(session.source, "continue");
    }

    #[test]
    fn test_session_from_jsonl_no_summary() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-def.jsonl");
        fs::write(
            &path,
            r#"{"cwd":"/dev/app","timestamp":1700000000000}
{"role":"user","content":"Hello","timestamp":1700000001000}
"#,
        )
        .unwrap();

        let session = session_from_jsonl(&path).unwrap();
        assert_eq!(session.name, "Continue session");
        assert_eq!(session.project_path, "/dev/app");
    }

    #[test]
    fn test_transcript_parse_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess.jsonl");
        fs::write(
            &path,
            r#"{"role":"user","content":"Hello","timestamp":1000}
{"role":"system","content":"ignored"}
{"role":"assistant","content":"Hi there!","timestamp":2000}
"#,
        )
        .unwrap();

        let msgs = transcript::parse_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn test_session_from_empty_jsonl() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("empty.jsonl");
        fs::write(&path, "").unwrap();
        // Empty file returns a session with defaults (mtime, "Continue session")
        let session = session_from_jsonl(&path);
        assert!(session.is_some());
        assert_eq!(session.unwrap().name, "Continue session");
    }

    #[test]
    fn test_session_from_corrupt_jsonl() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("corrupt.jsonl");
        fs::write(&path, "not json at all\n{also broken").unwrap();
        // Corrupt lines are skipped, no valid data extracted, still returns session
        let session = session_from_jsonl(&path);
        assert!(session.is_some());
        assert_eq!(session.unwrap().name, "Continue session");
    }
}
