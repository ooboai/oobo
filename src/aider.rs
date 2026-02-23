use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::cursor::transcript::Message;
use crate::cursor::Session;

const HISTORY_FILE: &str = ".aider.chat.history.md";

fn history_path(project_root: &str) -> PathBuf {
    Path::new(project_root).join(HISTORY_FILE)
}

fn session_id_from_path(path: &Path) -> String {
    let mut h = DefaultHasher::new();
    path.to_string_lossy().hash(&mut h);
    format!("aider-{:016x}", h.finish())
}

fn file_mtime_epoch(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64
        })
}

fn first_user_message(path: &Path) -> String {
    let content = fs::read_to_string(path).unwrap_or_default();
    for line in content.lines() {
        if line.starts_with("#### ") {
            let role = line.trim_start_matches("#### ").trim();
            if role.eq_ignore_ascii_case("user") || role.starts_with('/') {
                continue;
            }
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("#### ") {
            let name = if trimmed.len() > 60 {
                format!("{}…", &trimmed[..60])
            } else {
                trimmed.to_string()
            };
            return name;
        }
    }
    "aider chat".to_string()
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let path = history_path(project_root);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path).unwrap_or_default();
    let mut sessions = Vec::new();

    let mut current_start: Option<&str> = None;
    let mut first_msg: Option<String> = None;
    let mut msg_count: u32 = 0;

    for line in content.lines() {
        if line.starts_with("# aider chat started at ") {
            if let Some(ts) = current_start {
                sessions.push(build_session(
                    &path,
                    ts,
                    first_msg.take().unwrap_or_else(|| "aider chat".into()),
                    project_root,
                    sessions.len(),
                ));
            }
            current_start = Some(line.trim_start_matches("# aider chat started at ").trim());
            first_msg = None;
            msg_count = 0;
        } else if line.starts_with("#### ") && !line.starts_with("####  ") {
            msg_count += 1;
        } else if first_msg.is_none() && msg_count > 0 {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let name = if trimmed.len() > 60 {
                    format!("{}…", &trimmed[..60])
                } else {
                    trimmed.to_string()
                };
                first_msg = Some(name);
            }
        }
    }

    if let Some(ts) = current_start {
        sessions.push(build_session(
            &path,
            ts,
            first_msg.unwrap_or_else(|| "aider chat".into()),
            project_root,
            sessions.len(),
        ));
    }

    if sessions.is_empty() && path.exists() {
        sessions.push(Session {
            session_id: session_id_from_path(&path),
            name: first_user_message(&path),
            mode: "aider".to_string(),
            created_at: file_mtime_epoch(&path),
            updated_at: file_mtime_epoch(&path),
            project_path: project_root.to_string(),
            workspace_dir: project_root.to_string(),
            source: "aider".to_string(),
        });
    }

    sessions.reverse();
    Ok(sessions)
}

fn build_session(
    path: &Path,
    timestamp_str: &str,
    name: String,
    project_root: &str,
    index: usize,
) -> Session {
    let created = parse_aider_timestamp(timestamp_str);

    let mut h = DefaultHasher::new();
    path.to_string_lossy().hash(&mut h);
    index.hash(&mut h);
    let sid = format!("aider-{:016x}", h.finish());

    Session {
        session_id: sid,
        name,
        mode: "aider".to_string(),
        created_at: created,
        updated_at: created,
        project_path: project_root.to_string(),
        workspace_dir: project_root.to_string(),
        source: "aider".to_string(),
    }
}

fn parse_aider_timestamp(s: &str) -> Option<i64> {
    // "2024-01-15 14:30:00"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    let time_parts: Vec<&str> = parts[1].split(':').collect();
    if date_parts.len() < 3 || time_parts.len() < 3 {
        return None;
    }

    let y: i32 = date_parts[0].parse().ok()?;
    let mo: u32 = date_parts[1].parse().ok()?;
    let d: u32 = date_parts[2].parse().ok()?;
    let h: u32 = time_parts[0].parse().ok()?;
    let mi: u32 = time_parts[1].parse().ok()?;
    let s: u32 = time_parts[2].parse().ok()?;

    let days = days_from_epoch(y, mo, d)?;
    let secs = days as i64 * 86400 + h as i64 * 3600 + mi as i64 * 60 + s as i64;
    Some(secs * 1000)
}

fn days_from_epoch(y: i32, m: u32, d: u32) -> Option<i64> {
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    let y_adj = if m <= 2 { y - 1 } else { y };
    let m_adj = if m <= 2 { m + 9 } else { m - 3 };
    let era = y_adj / 400;
    let yoe = y_adj - era * 400;
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let civil = era * 146097 + doe - 719468;
    Some(civil)
}

#[allow(dead_code)]
pub fn all_sessions() -> Result<Vec<Session>, String> {
    let project_root = crate::cursor::get_project_root();
    sessions_for_project(&project_root)
}

pub mod transcript {
    use super::*;

    pub fn find_transcript_path(project_path: &str, _session_id: &str) -> Option<PathBuf> {
        let p = history_path(project_path);
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    pub fn count_messages(project_path: &str, _session_id: &str) -> u32 {
        let p = history_path(project_path);
        if !p.exists() {
            return 0;
        }
        let content = fs::read_to_string(&p).unwrap_or_default();
        content.lines().filter(|l| l.starts_with("#### ")).count() as u32
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let content = fs::read_to_string(path).unwrap_or_default();
        let mut messages: Vec<Message> = Vec::new();
        let mut current_role: Option<&str> = None;
        let mut current_text = String::new();

        for line in content.lines() {
            if line.starts_with("# aider chat started at ") {
                if let Some(role) = current_role.take() {
                    if !current_text.trim().is_empty() {
                        messages.push(Message {
                            role: role.to_string(),
                            text: current_text.trim().to_string(),
                        });
                    }
                }
                current_text.clear();
                continue;
            }

            if line.starts_with("#### ") {
                if let Some(role) = current_role.take() {
                    if !current_text.trim().is_empty() {
                        messages.push(Message {
                            role: role.to_string(),
                            text: current_text.trim().to_string(),
                        });
                    }
                }
                current_text.clear();

                let role_str = line.trim_start_matches("#### ").trim();
                current_role = Some(
                    if role_str.eq_ignore_ascii_case("user") || role_str.starts_with('/') {
                        "user"
                    } else {
                        "assistant"
                    },
                );

                if role_str.starts_with('/') {
                    current_text.push_str(role_str);
                    current_text.push('\n');
                }
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
    fn test_parse_aider_history() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history = tmp.path().join(HISTORY_FILE);
        fs::write(
            &history,
            "# aider chat started at 2024-06-01 10:00:00\n\n\
             #### user\nWrite hello world\n\n\
             #### assistant\nHere it is:\n```python\nprint('hello')\n```\n\n\
             # aider chat started at 2024-06-02 14:00:00\n\n\
             #### user\nAdd tests\n\n\
             #### assistant\nDone.\n",
        )
        .unwrap();

        let sessions = sessions_for_project(&tmp.path().to_string_lossy()).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].source, "aider");
    }

    #[test]
    fn test_parse_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history = tmp.path().join("chat.md");
        fs::write(
            &history,
            "# aider chat started at 2024-01-01 00:00:00\n\n\
             #### user\nHello\n\n\
             #### assistant\nHi there!\n",
        )
        .unwrap();

        let msgs = transcript::parse_messages(&history);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "Hello");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].text, "Hi there!");
    }
}
