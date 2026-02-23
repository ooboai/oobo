use std::fs;
use std::path::{Path, PathBuf};

use crate::cursor::transcript::Message;
use crate::cursor::Session;

fn codex_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex"))
}

fn sessions_dir() -> Option<PathBuf> {
    codex_dir().map(|d| d.join("sessions"))
}

/// Parse a rollout JSONL file to extract session metadata.
fn session_from_rollout(path: &Path) -> Option<Session> {
    let content = fs::read_to_string(path).ok()?;
    let filename = path.file_stem()?.to_str()?;

    let session_id = filename
        .strip_prefix("rollout-")
        .unwrap_or(filename)
        .to_string();

    let mut name = String::new();
    let mut created_at: Option<i64> = None;
    let mut updated_at: Option<i64> = None;
    let mut project_path = String::new();

    for line in content.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_iso_timestamp)
            .or_else(|| v.get("timestamp").and_then(|t| t.as_i64()));

        if created_at.is_none() {
            created_at = ts;
        }
        if ts.is_some() {
            updated_at = ts;
        }

        let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if event_type == "session_start" {
            if let Some(cwd) = v
                .get("payload")
                .and_then(|p| p.get("cwd"))
                .and_then(|c| c.as_str())
            {
                project_path = cwd.to_string();
            }
        }

        if name.is_empty() && event_type == "event_msg" {
            if let Some(msg_type) = v
                .get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
            {
                if msg_type == "user_message" {
                    if let Some(text) = v
                        .get("payload")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        name = if text.len() > 60 {
                            format!("{}…", &text[..60])
                        } else {
                            text.to_string()
                        };
                    }
                }
            }
        }

        if !name.is_empty() && created_at.is_some() && !project_path.is_empty() {
            break;
        }
    }

    if name.is_empty() {
        name = "Codex session".to_string();
    }

    Some(Session {
        session_id,
        name,
        mode: "codex".to_string(),
        created_at,
        updated_at,
        project_path,
        workspace_dir: String::new(),
        source: "codex".to_string(),
    })
}

fn parse_iso_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let date_part = &s[..10];
    let time_part = if s.len() >= 19 {
        &s[11..19]
    } else {
        return None;
    };

    let dp: Vec<&str> = date_part.split('-').collect();
    let tp: Vec<&str> = time_part.split(':').collect();
    if dp.len() < 3 || tp.len() < 3 {
        return None;
    }

    let y: i64 = dp[0].parse().ok()?;
    let mo: i64 = dp[1].parse().ok()?;
    let d: i64 = dp[2].parse().ok()?;
    let h: i64 = tp[0].parse().ok()?;
    let mi: i64 = tp[1].parse().ok()?;
    let sec: i64 = tp[2].parse().ok()?;

    let y_adj = if mo <= 2 { y - 1 } else { y };
    let m_adj = if mo <= 2 { mo + 9 } else { mo - 3 };
    let era = y_adj / 400;
    let yoe = y_adj - era * 400;
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + sec;
    Some(secs * 1000)
}

/// Recursively find all rollout JSONL files under ~/.codex/sessions/.
fn find_all_rollouts() -> Vec<PathBuf> {
    let dir = match sessions_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Vec::new(),
    };
    let mut files = Vec::new();
    collect_rollouts(&dir, &mut files);
    files.sort();
    files.reverse();
    files
}

fn collect_rollouts(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_rollouts(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("rollout-") {
                out.push(p);
            }
        }
    }
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let norm = normalize(project_root);
    let mut sessions: Vec<Session> = find_all_rollouts()
        .iter()
        .filter_map(|p| session_from_rollout(p))
        .filter(|s| !s.project_path.is_empty() && normalize(&s.project_path) == norm)
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let mut sessions: Vec<Session> = find_all_rollouts()
        .iter()
        .filter_map(|p| session_from_rollout(p))
        .collect();
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
        let dir = sessions_dir()?;
        find_rollout_by_id(&dir, session_id)
    }

    fn find_rollout_by_id(dir: &Path, session_id: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(found) = find_rollout_by_id(&p, session_id) {
                    return Some(found);
                }
            } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let id = stem.strip_prefix("rollout-").unwrap_or(stem);
                if id == session_id || id.starts_with(session_id) {
                    return Some(p);
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
        let mut messages = Vec::new();

        for line in content.lines() {
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

            if event_type == "event_msg" {
                let msg_type = v
                    .get("payload")
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");

                if msg_type == "user_message" {
                    if let Some(text) = v
                        .get("payload")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        messages.push(Message {
                            role: "user".to_string(),
                            text: text.to_string(),
                        });
                    }
                }
            } else if event_type == "response_item" {
                let role = v
                    .get("payload")
                    .and_then(|p| p.get("role"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("assistant");

                let text = extract_response_text(&v);
                if !text.is_empty() {
                    messages.push(Message {
                        role: role.to_string(),
                        text,
                    });
                }
            }
        }

        messages
    }

    fn extract_response_text(v: &serde_json::Value) -> String {
        if let Some(content) = v
            .get("payload")
            .and_then(|p| p.get("content"))
            .and_then(|c| c.as_array())
        {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                .collect();
            if !texts.is_empty() {
                return texts.join("\n");
            }
        }

        if let Some(text) = v
            .get("payload")
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
        {
            return text.to_string();
        }

        String::new()
    }

    pub fn extract_stats(path: &Path) -> Option<crate::server::payload::SessionStats> {
        let content = fs::read_to_string(path).ok()?;
        let mut files_touched: Vec<String> = Vec::new();
        let mut tool_call_count: u32 = 0;
        let mut first_ts: Option<i64> = None;
        let mut last_ts: Option<i64> = None;

        for line in content.lines() {
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let ts = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_iso_timestamp)
                .or_else(|| v.get("timestamp").and_then(|t| t.as_i64()));

            if let Some(t) = ts {
                if first_ts.is_none() {
                    first_ts = Some(t);
                }
                last_ts = Some(t);
            }

            let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

            if event_type == "response_item" {
                if let Some(payload) = v.get("payload") {
                    let item_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if item_type == "function_call" || item_type == "tool_call" {
                        tool_call_count += 1;
                        if let Some(args) = payload
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                        {
                            for key in ["path", "file_path", "file"] {
                                if let Some(fp) = args.get(key).and_then(|v| v.as_str()) {
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

        let duration_secs = match (first_ts, last_ts) {
            (Some(f), Some(l)) if l > f => Some(((l - f) / 1000) as u64),
            _ => None,
        };

        Some(crate::server::payload::SessionStats {
            model: None,
            input_tokens: None,
            output_tokens: None,
            total_cost_usd: None,
            duration_secs,
            files_touched,
            tool_call_count,
        })
    }

    pub fn stats_for_session(
        _project_path: &str,
        session_id: &str,
    ) -> Option<crate::server::payload::SessionStats> {
        let path = find_transcript_path("", session_id)?;
        extract_stats(&path)
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
    fn test_parse_codex_rollout() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rollout = tmp.path().join("rollout-2025-06-01T10-00-00-abc123.jsonl");
        fs::write(
            &rollout,
            r#"{"type":"session_start","timestamp":"2025-06-01T10:00:00Z","payload":{"cwd":"/home/dev/project"}}
{"type":"event_msg","timestamp":"2025-06-01T10:00:01Z","payload":{"type":"user_message","message":"Fix the login bug"}}
{"type":"response_item","timestamp":"2025-06-01T10:00:05Z","payload":{"role":"assistant","content":[{"type":"text","text":"I'll look into the login handler..."}]}}
{"type":"event_msg","timestamp":"2025-06-01T10:00:10Z","payload":{"type":"user_message","message":"Looks good, ship it"}}
{"type":"response_item","timestamp":"2025-06-01T10:00:15Z","payload":{"role":"assistant","content":[{"type":"text","text":"Done! The fix has been applied."}]}}
"#,
        )
        .unwrap();

        let session = session_from_rollout(&rollout).unwrap();
        assert_eq!(session.source, "codex");
        assert_eq!(session.name, "Fix the login bug");
        assert_eq!(session.project_path, "/home/dev/project");

        let msgs = transcript::parse_messages(&rollout);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Fix the login bug");
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].text.contains("login handler"));
        assert_eq!(msgs[2].role, "user");
        assert_eq!(msgs[3].role, "assistant");
    }

    #[test]
    fn test_parse_iso_timestamp() {
        let ts = parse_iso_timestamp("2025-06-01T10:00:00Z");
        assert!(ts.is_some());
        assert!(ts.unwrap() > 0);
    }

    #[test]
    fn test_empty_rollout() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rollout = tmp.path().join("rollout-empty.jsonl");
        fs::write(&rollout, "").unwrap();

        let session = session_from_rollout(&rollout);
        assert!(session.is_some());
        assert_eq!(session.unwrap().name, "Codex session");
    }
}
