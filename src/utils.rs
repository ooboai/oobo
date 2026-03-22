use std::path::Path;

use crate::tools::cursor::transcript::Message;

pub const MAX_SESSION_NAME_LEN: usize = 60;

/// Normalize any epoch timestamp (seconds, milliseconds, or microseconds) to seconds.
/// Thresholds use strict `>=` to correctly classify boundary values:
/// - >= 1e15 → microseconds
/// - >= 1e12 → milliseconds (year ~2001 in seconds, but all real ms timestamps are >= ~1e12)
/// - otherwise → seconds
pub fn to_epoch_secs(ts: i64) -> i64 {
    if ts >= 1_000_000_000_000_000 {
        ts / 1_000_000
    } else if ts >= 1_000_000_000_000 {
        ts / 1_000
    } else {
        ts
    }
}

/// Truncate a session name to a reasonable display length.
/// Safe for multi-byte UTF-8 — never slices mid-codepoint.
pub fn truncate_name(text: &str, max_len: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= max_len {
        return trimmed.to_string();
    }
    let end = trimmed
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= max_len)
        .last()
        .unwrap_or(0);
    format!("{}…", &trimmed[..end])
}

/// Parse an ISO 8601 timestamp string into epoch milliseconds.
/// Handles formats like `2026-03-05T10:00:00Z` and `2026-03-05T10:00:00.000Z`.
pub fn parse_iso_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let date_part = &s[..10];
    let time_part = &s[11..19];

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

/// Format a slice of messages as a human-readable transcript.
/// Shows the last `max_messages` messages with role labels.
pub fn format_transcript(messages: &[Message], max_messages: u32, assistant_label: &str) -> String {
    let start = messages.len().saturating_sub(max_messages as usize);
    let mut out = String::new();
    for msg in &messages[start..] {
        let label = if msg.role == "user" {
            "User"
        } else {
            assistant_label
        };
        out.push_str(&format!("── {label} ──\n{}\n\n", msg.text));
    }
    out
}

/// Sanitize a value for pipe-delimited agent output.
/// Replaces `|` with `/` and collapses whitespace to single spaces.
pub fn sanitize_pipe(s: &str) -> String {
    s.replace('|', "/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// Open a SQLite database in read-only mode with no-mutex flag.
pub fn open_db_readonly(path: &Path) -> Result<rusqlite::Connection, rusqlite::Error> {
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_pipe_no_pipes() {
        assert_eq!(sanitize_pipe("hello world"), "hello world");
    }

    #[test]
    fn test_sanitize_pipe_with_pipes() {
        assert_eq!(sanitize_pipe("fix: handle X | Y case"), "fix: handle X / Y case");
    }

    #[test]
    fn test_sanitize_pipe_collapses_whitespace() {
        assert_eq!(sanitize_pipe("hello\n  world"), "hello world");
    }

    #[test]
    fn test_sanitize_pipe_combined() {
        assert_eq!(sanitize_pipe("a | b\nc"), "a / b c");
    }

    #[test]
    fn test_truncate_name_short() {
        assert_eq!(truncate_name("hello", 60), "hello");
    }

    #[test]
    fn test_truncate_name_exact() {
        let s = "a".repeat(60);
        assert_eq!(truncate_name(&s, 60), s);
    }

    #[test]
    fn test_truncate_name_long() {
        let s = "a".repeat(80);
        let result = truncate_name(&s, 60);
        assert!(result.ends_with('…'));
        assert_eq!(result.len(), 60 + '…'.len_utf8());
    }

    #[test]
    fn test_truncate_name_whitespace() {
        assert_eq!(truncate_name("  hello  ", 60), "hello");
    }

    #[test]
    fn test_parse_iso_timestamp_valid() {
        let ts = parse_iso_timestamp("2026-03-05T10:30:00Z").unwrap();
        assert!(ts > 0);
        assert_eq!(ts, 1772706600000);
    }

    #[test]
    fn test_parse_iso_timestamp_with_millis() {
        let ts = parse_iso_timestamp("2026-03-05T10:30:00.123Z").unwrap();
        assert_eq!(ts, 1772706600000);
    }

    #[test]
    fn test_parse_iso_timestamp_too_short() {
        assert!(parse_iso_timestamp("2026-03").is_none());
    }

    #[test]
    fn test_parse_iso_timestamp_invalid() {
        assert!(parse_iso_timestamp("not-a-date-at-all").is_none());
    }

    #[test]
    fn test_format_transcript_empty() {
        assert_eq!(format_transcript(&[], 10, "Assistant"), "");
    }

    #[test]
    fn test_format_transcript_basic() {
        let msgs = vec![
            Message {
                role: "user".into(),
                text: "hello".into(),
                timestamp_ms: None,
            },
            Message {
                role: "assistant".into(),
                text: "hi".into(),
                timestamp_ms: None,
            },
        ];
        let out = format_transcript(&msgs, 10, "Assistant");
        assert!(out.contains("── User ──"));
        assert!(out.contains("── Assistant ──"));
        assert!(out.contains("hello"));
        assert!(out.contains("hi"));
    }

    #[test]
    fn test_format_transcript_max_messages() {
        let msgs: Vec<Message> = (0..10)
            .map(|i| Message {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                text: format!("msg{i}"),
                timestamp_ms: None,
            })
            .collect();
        let out = format_transcript(&msgs, 3, "Bot");
        assert!(!out.contains("msg0"));
        assert!(out.contains("msg7"));
        assert!(out.contains("msg8"));
        assert!(out.contains("msg9"));
    }

    #[test]
    fn test_format_transcript_custom_label() {
        let msgs = vec![Message {
            role: "assistant".into(),
            text: "test".into(),
            timestamp_ms: None,
        }];
        let out = format_transcript(&msgs, 10, "Gemini");
        assert!(out.contains("── Gemini ──"));
    }

    #[test]
    fn test_open_db_readonly_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);")
            .unwrap();
        drop(conn);

        let ro = open_db_readonly(&db_path).unwrap();
        let val: i32 = ro.query_row("SELECT id FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(val, 1);
    }

    #[test]
    fn test_open_db_readonly_cannot_write() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("readonly.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
            .unwrap();
        drop(conn);

        let ro = open_db_readonly(&db_path).unwrap();
        let result = ro.execute_batch("INSERT INTO t VALUES (1)");
        assert!(result.is_err(), "read-only connection should reject writes");
    }

    #[test]
    fn test_open_db_readonly_nonexistent() {
        let result = open_db_readonly(Path::new("/tmp/nonexistent-oobo-db-xyz.db"));
        assert!(result.is_err());
    }

    #[test]
    fn test_truncate_name_multibyte_utf8() {
        let result = truncate_name("café", 3);
        assert!(result.starts_with("caf"));
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_name_emoji() {
        let result = truncate_name("hi 🚀 there", 4);
        assert!(result.ends_with('…'));
        assert!(!result.contains('\u{FFFD}'));
    }

    #[test]
    fn test_truncate_name_all_multibyte() {
        let result = truncate_name("日本語テスト", 4);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_name_zero_max() {
        let result = truncate_name("hello", 0);
        assert_eq!(result, "…");
    }

    #[test]
    fn test_to_epoch_secs_seconds() {
        assert_eq!(to_epoch_secs(1_772_000_000), 1_772_000_000);
    }

    #[test]
    fn test_to_epoch_secs_milliseconds() {
        assert_eq!(to_epoch_secs(1_772_000_000_000), 1_772_000_000);
    }

    #[test]
    fn test_to_epoch_secs_microseconds() {
        assert_eq!(to_epoch_secs(1_772_000_000_000_000), 1_772_000_000);
    }

    #[test]
    fn test_to_epoch_secs_zero() {
        assert_eq!(to_epoch_secs(0), 0);
    }

    #[test]
    fn test_to_epoch_secs_boundary_milliseconds() {
        // Exactly 1e12 is a millisecond timestamp (year ~2001), not seconds
        assert_eq!(to_epoch_secs(1_000_000_000_000), 1_000_000_000);
    }

    #[test]
    fn test_to_epoch_secs_boundary_microseconds() {
        assert_eq!(to_epoch_secs(1_000_000_000_000_000), 1_000_000_000);
    }
}

/// Truncate a string to `max` characters at a safe UTF-8 boundary.
/// Appends "..." if truncation occurs.
pub fn truncate_str(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

/// Extract a short summary from a tool's input JSON.
/// Picks the most relevant field per tool type and truncates to `max_len`.
pub fn summarize_tool_input(
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
    max_len: usize,
) -> Option<String> {
    let ti = tool_input?;
    let raw = match tool_name {
        "Bash" | "Shell" => ti.get("command").and_then(|v| v.as_str()),
        "Write" | "Read" | "Edit" | "MultiEdit" | "Delete" | "StrReplace" | "ReadNotebook"
        | "EditNotebook" => ti
            .get("file_path")
            .or_else(|| ti.get("path"))
            .and_then(|v| v.as_str()),
        "Grep" | "Glob" | "codebase_search" | "file_search" | "SemanticSearch" => ti
            .get("pattern")
            .or_else(|| ti.get("query"))
            .and_then(|v| v.as_str()),
        "WebFetch" => ti.get("url").and_then(|v| v.as_str()),
        "WebSearch" => ti.get("query").and_then(|v| v.as_str()),
        "Agent" | "Task" => ti
            .get("description")
            .or_else(|| ti.get("task"))
            .or_else(|| ti.get("prompt"))
            .and_then(|v| v.as_str()),
        _ => ti
            .get("command")
            .or_else(|| ti.get("file_path"))
            .or_else(|| ti.get("path"))
            .or_else(|| ti.get("pattern"))
            .or_else(|| ti.get("query"))
            .and_then(|v| v.as_str()),
    }?;
    Some(truncate_str(raw, max_len))
}

#[cfg(test)]
mod truncate_tests {
    use super::*;

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        assert_eq!(truncate_str("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_str_unicode() {
        let s = "こんにちは世界"; // 7 chars
        let result = truncate_str(s, 3);
        assert_eq!(result, "こんに...");
    }

    #[test]
    fn test_truncate_str_emoji() {
        let s = "hello 🌍🌎🌏 world";
        let result = truncate_str(s, 8);
        assert_eq!(result, "hello 🌍🌎...");
    }
}
