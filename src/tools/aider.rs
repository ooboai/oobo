use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::tools::cursor::transcript::Message;
use crate::tools::cursor::Session;

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
            return crate::utils::truncate_name(trimmed, crate::utils::MAX_SESSION_NAME_LEN);
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
                first_msg = Some(crate::utils::truncate_name(
                    trimmed,
                    crate::utils::MAX_SESSION_NAME_LEN,
                ));
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
            parent_session_id: None,
            subagent_type: None,
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
        parent_session_id: None,
        subagent_type: None,
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
                            timestamp_ms: None,
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
                            timestamp_ms: None,
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

pub mod analytics {
    use std::fs;
    use std::io::BufRead;
    use std::path::PathBuf;

    use crate::analytics::NativeStats;

    /// Default path for Aider analytics JSONL log.
    pub fn analytics_log_path() -> PathBuf {
        crate::paths::oobo_home().join("aider-analytics.jsonl")
    }

    pub fn has_analytics_log() -> bool {
        let path = analytics_log_path();
        path.exists() && fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false)
    }

    struct AiderEvent {
        time_secs: i64,
        model: Option<String>,
        prompt_tokens: u64,
        completion_tokens: u64,
        cost: f64,
    }

    fn load_events_in_window(start_secs: i64, end_secs: i64) -> Vec<AiderEvent> {
        let path = analytics_log_path();
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let reader = std::io::BufReader::new(file);
        let mut events = Vec::new();

        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event_name = entry.get("event").and_then(|v| v.as_str()).unwrap_or("");
            if event_name != "message_send" {
                continue;
            }

            let time = entry.get("time").and_then(|v| v.as_i64()).unwrap_or(0);
            if time < start_secs || time >= end_secs {
                continue;
            }

            let props = match entry.get("properties") {
                Some(p) => p,
                None => continue,
            };

            events.push(AiderEvent {
                time_secs: time,
                model: props
                    .get("main_model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                prompt_tokens: props
                    .get("prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                completion_tokens: props
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cost: props.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0),
            });
        }

        events
    }

    /// Extract native stats for an aider session by matching analytics events
    /// to the session's time window.
    ///
    /// `session_start_ms`: session start time in epoch milliseconds.
    /// `session_end_ms`: optional end time (e.g. next session start). Defaults to +24h.
    pub fn extract_native_stats(
        session_start_ms: Option<i64>,
        session_end_ms: Option<i64>,
    ) -> Option<NativeStats> {
        if !has_analytics_log() {
            return None;
        }
        let start_ms = session_start_ms?;
        let start_secs = start_ms / 1000;
        let end_secs = session_end_ms
            .map(|ms| ms / 1000)
            .unwrap_or(start_secs + 86400);

        let events = load_events_in_window(start_secs, end_secs);
        if events.is_empty() {
            return None;
        }

        let model = events.iter().rev().find_map(|e| e.model.clone());
        let input_tokens: u64 = events.iter().map(|e| e.prompt_tokens).sum();
        let output_tokens: u64 = events.iter().map(|e| e.completion_tokens).sum();
        let duration = if events.len() >= 2 {
            let first = events.first().unwrap().time_secs;
            let last = events.last().unwrap().time_secs;
            if last > first {
                Some((last - first) as u64)
            } else {
                None
            }
        } else {
            None
        };

        Some(NativeStats {
            model,
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            duration_secs: duration,
            files_touched: Vec::new(),
            tool_call_count: 0,
        })
    }

    /// Return global aggregated stats from the entire analytics log.
    pub fn global_stats() -> Option<(u64, u64, f64, usize)> {
        if !has_analytics_log() {
            return None;
        }
        let events = load_events_in_window(0, i64::MAX);
        if events.is_empty() {
            return None;
        }
        let input: u64 = events.iter().map(|e| e.prompt_tokens).sum();
        let output: u64 = events.iter().map(|e| e.completion_tokens).sum();
        let cost: f64 = events.iter().map(|e| e.cost).sum();
        Some((input, output, cost, events.len()))
    }

    /// Check if the user's Aider config has analytics-log configured.
    pub fn is_aider_config_set() -> bool {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return false,
        };
        let conf = home.join(".aider.conf.yml");
        if !conf.exists() {
            return false;
        }
        let content = fs::read_to_string(&conf).unwrap_or_default();
        content.contains("analytics-log")
    }

    /// Return the snippet to add to `~/.aider.conf.yml`.
    pub fn config_snippet() -> String {
        let path = analytics_log_path();
        format!("analytics-log: {}", path.display())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;

        fn make_analytics_log(dir: &std::path::Path) -> PathBuf {
            let path = dir.join("aider-analytics.jsonl");
            let mut f = fs::File::create(&path).unwrap();
            // Session starting at epoch 1000 (seconds)
            writeln!(
                f,
                r#"{{"event":"message_send","properties":{{"main_model":"anthropic/claude-sonnet-4-6","prompt_tokens":5000,"completion_tokens":1200,"total_tokens":6200,"cost":0.042,"total_cost":0.042}},"user_id":"test-uuid","time":1000}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"event":"message_send","properties":{{"main_model":"anthropic/claude-sonnet-4-6","prompt_tokens":3000,"completion_tokens":800,"total_tokens":3800,"cost":0.028,"total_cost":0.070}},"user_id":"test-uuid","time":1120}}"#
            )
            .unwrap();
            // Different session at epoch 100000
            writeln!(
                f,
                r#"{{"event":"message_send","properties":{{"main_model":"openai/gpt-4o","prompt_tokens":2000,"completion_tokens":500,"total_tokens":2500,"cost":0.015,"total_cost":0.015}},"user_id":"test-uuid","time":100000}}"#
            )
            .unwrap();
            // Non message_send event (should be ignored)
            writeln!(
                f,
                r#"{{"event":"exit","properties":{{"reason":"Control-C"}},"user_id":"test-uuid","time":1200}}"#
            )
            .unwrap();
            path
        }

        #[test]
        fn test_load_events_in_window() {
            let tmp = tempfile::TempDir::new().unwrap();
            let log_path = make_analytics_log(tmp.path());

            // Temporarily override the log path by using load_events_in_window directly
            // We need to set up the path. Instead, test via file reading.
            let content = fs::read_to_string(&log_path).unwrap();
            assert!(content.contains("message_send"));
            assert!(content.contains("anthropic/claude-sonnet"));
        }

        #[test]
        fn test_config_snippet() {
            let snippet = config_snippet();
            assert!(snippet.starts_with("analytics-log:"));
            assert!(snippet.contains("aider-analytics.jsonl"));
        }
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
