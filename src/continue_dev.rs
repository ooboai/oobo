use std::fs;
use std::path::{Path, PathBuf};

use crate::cursor::transcript::Message;
use crate::cursor::Session;

fn continue_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".continue"))
}

fn sessions_dir() -> Option<PathBuf> {
    continue_dir().map(|d| d.join("sessions"))
}

fn session_transcripts_dir() -> Option<PathBuf> {
    continue_dir().map(|d| d.join("session-transcripts"))
}

fn parse_session_json(path: &Path) -> Option<Session> {
    let content = fs::read_to_string(path).ok()?;
    let data: serde_json::Value = serde_json::from_str(&content).ok()?;

    let session_id = data
        .get("sessionId")
        .and_then(|v| v.as_str())
        .or_else(|| path.file_stem().and_then(|s| s.to_str()))?
        .to_string();

    let title = data
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Continue session")
        .to_string();

    let created_at = data
        .get("dateCreated")
        .and_then(|v| v.as_str())
        .and_then(parse_iso_timestamp)
        .or_else(|| data.get("dateCreated").and_then(|v| v.as_i64()));

    let workspace = data
        .get("workspaceDirectory")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(Session {
        session_id,
        name: title,
        mode: "continue".to_string(),
        created_at,
        updated_at: created_at,
        project_path: workspace.clone(),
        workspace_dir: workspace,
        source: "continue".to_string(),
    })
}

fn parse_iso_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let y: i32 = s[..4].parse().ok()?;
    let mo: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    let h: u32 = s[11..13].parse().ok()?;
    let mi: u32 = s[14..16].parse().ok()?;
    let sec: u32 = s[17..19].parse().ok()?;

    let y_adj = if mo <= 2 { y as i64 - 1 } else { y as i64 };
    let m_adj = if mo <= 2 {
        mo as i64 + 9
    } else {
        mo as i64 - 3
    };
    let era = y_adj / 400;
    let yoe = y_adj - era * 400;
    let doy = (153 * m_adj + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h as i64 * 3600 + mi as i64 * 60 + sec as i64;
    Some(secs * 1000)
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let mut sessions = all_sessions()?;
    let norm = normalize(project_root);
    sessions.retain(|s| !s.project_path.is_empty() && normalize(&s.project_path) == norm);
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let mut sessions = Vec::new();

    if let Some(dir) = sessions_dir() {
        if dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Some(s) = parse_session_json(&p) {
                            sessions.push(s);
                        }
                    }
                }
            }
        }
    }

    if let Some(dir) = session_transcripts_dir() {
        if dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("md") {
                        let session_id = p
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();

                        let already = sessions.iter().any(|s| s.session_id == session_id);
                        if already {
                            continue;
                        }

                        let mtime =
                            fs::metadata(&p)
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .map(|t| {
                                    t.duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as i64
                                });

                        sessions.push(Session {
                            session_id,
                            name: "Continue transcript".to_string(),
                            mode: "continue".to_string(),
                            created_at: mtime,
                            updated_at: mtime,
                            project_path: String::new(),
                            workspace_dir: String::new(),
                            source: "continue".to_string(),
                        });
                    }
                }
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

fn normalize(p: &str) -> String {
    match fs::canonicalize(p) {
        Ok(c) => c.to_string_lossy().to_string(),
        Err(_) => p.trim_end_matches('/').to_string(),
    }
}

pub mod transcript {
    use super::*;

    pub fn find_transcript_path(_project_path: &str, session_id: &str) -> Option<PathBuf> {
        if let Some(dir) = sessions_dir() {
            let json = dir.join(format!("{session_id}.json"));
            if json.exists() {
                return Some(json);
            }
        }

        if let Some(dir) = session_transcripts_dir() {
            let md = dir.join(format!("{session_id}.md"));
            if md.exists() {
                return Some(md);
            }
        }

        None
    }

    pub fn count_messages(_project_path: &str, session_id: &str) -> u32 {
        match find_transcript_path("", session_id) {
            Some(p) => {
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    count_json_messages(&p)
                } else {
                    count_md_messages(&p)
                }
            }
            None => 0,
        }
    }

    fn count_json_messages(path: &Path) -> u32 {
        let content = fs::read_to_string(path).unwrap_or_default();
        let data: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        data.get("history")
            .and_then(|v| v.as_array())
            .map(|a| a.len() as u32)
            .unwrap_or(0)
    }

    fn count_md_messages(path: &Path) -> u32 {
        let content = fs::read_to_string(path).unwrap_or_default();
        content
            .lines()
            .filter(|l| l.starts_with("## ") || l.starts_with("### "))
            .count() as u32
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            parse_json_messages(path)
        } else {
            parse_md_messages(path)
        }
    }

    pub fn parse_json_messages(path: &Path) -> Vec<Message> {
        let content = fs::read_to_string(path).unwrap_or_default();
        let data: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let history = match data.get("history").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return Vec::new(),
        };

        let mut messages = Vec::new();
        for entry in history {
            let msg = match entry.get("message") {
                Some(m) => m,
                None => entry,
            };
            let role = msg
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let text = msg
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !text.is_empty() {
                messages.push(Message { role, text });
            }
        }
        messages
    }

    fn parse_md_messages(path: &Path) -> Vec<Message> {
        let content = fs::read_to_string(path).unwrap_or_default();
        let mut messages = Vec::new();
        let mut current_role: Option<String> = None;
        let mut current_text = String::new();

        for line in content.lines() {
            if line.starts_with("## ") || line.starts_with("### ") {
                if let Some(role) = current_role.take() {
                    if !current_text.trim().is_empty() {
                        messages.push(Message {
                            role,
                            text: current_text.trim().to_string(),
                        });
                    }
                }
                current_text.clear();

                let header = line.trim_start_matches('#').trim().to_lowercase();
                current_role = Some(if header.contains("user") || header.contains("human") {
                    "user".to_string()
                } else {
                    "assistant".to_string()
                });
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
                    role,
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
    fn test_parse_session_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_file = tmp.path().join("sess-1.json");
        fs::write(
            &session_file,
            r#"{
                "sessionId": "sess-1",
                "title": "Fix bugs",
                "dateCreated": "2024-06-15T10:30:00Z",
                "workspaceDirectory": "/home/dev/proj",
                "history": [
                    {"message": {"role": "user", "content": "Fix the bug"}},
                    {"message": {"role": "assistant", "content": "Done!"}}
                ]
            }"#,
        )
        .unwrap();

        let session = parse_session_json(&session_file).unwrap();
        assert_eq!(session.session_id, "sess-1");
        assert_eq!(session.name, "Fix bugs");
        assert_eq!(session.source, "continue");

        let msgs = transcript::parse_json_messages(&session_file);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }
}
