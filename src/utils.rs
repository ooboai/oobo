use std::path::Path;

use crate::tools::cursor::transcript::Message;

pub const MAX_SESSION_NAME_LEN: usize = 60;

/// Normalize any epoch timestamp (seconds, milliseconds, or microseconds) to seconds.
#[cfg(test)]
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
    use proptest::prelude::*;

    fn duration_multiplier(suffix: &str) -> i64 {
        match suffix {
            "s" => 1,
            "m" => 60,
            "h" => 3_600,
            "d" => 86_400,
            "w" => 7 * 86_400,
            "mo" => 30 * 86_400,
            "y" => 365 * 86_400,
            _ => unreachable!("test suffix set is fixed"),
        }
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

    #[test]
    fn test_parse_since_accepts_all_duration_suffixes() {
        for raw in ["0s", "1m", "2h", "3d", "4w", "5mo", "6y"] {
            assert!(parse_since(raw).is_ok(), "{raw} should parse");
        }
    }

    #[test]
    fn test_parse_since_rejects_bad_or_overflowing_duration() {
        assert!(parse_since("").is_err());
        assert!(parse_since("-1d").is_err());
        assert!(parse_since("10fortnights").is_err());
        assert!(parse_since("999999999999999999999999999999999999y").is_err());
    }

    proptest! {
        #[test]
        fn parse_since_duration_suffixes_match_expected_seconds(
            n in 0_i64..1_000_000,
            suffix in prop::sample::select(vec!["s", "m", "h", "d", "w", "mo", "y"]),
        ) {
            let raw = format!("{n}{suffix}");
            let before = chrono::Utc::now().timestamp();
            let cutoff = parse_since(&raw).expect("generated duration should parse");
            let after = chrono::Utc::now().timestamp();
            let expected = duration_multiplier(suffix) * n;

            prop_assert!(before - cutoff >= expected);
            prop_assert!(after - cutoff <= expected + 1);
        }

        #[test]
        fn parse_since_rejects_non_duration_garbage(raw in "[A-Za-z_./:-]{1,64}") {
            prop_assume!(chrono::DateTime::parse_from_rfc3339(&raw).is_err());
            prop_assert!(parse_since(&raw).is_err());
        }
    }
}

/// Format epoch-seconds as a relative time string (e.g. "5m", "3h", "2d", "1w", "3mo", "1y").
pub fn relative_time(ts: i64) -> String {
    if ts <= 0 {
        return "-".to_string();
    }
    let now = chrono::Utc::now().timestamp();
    let d = (now - ts).max(0);
    if d < 60 {
        format!("{d}s")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else if d < 7 * 86400 {
        format!("{}d", d / 86400)
    } else if d < 30 * 86400 {
        format!("{}w", d / (7 * 86400))
    } else if d < 365 * 86400 {
        format!("{}mo", d / (30 * 86400))
    } else {
        format!("{}y", d / (365 * 86400))
    }
}

/// Format a token count for human display (e.g. "1.2M", "45k", "800").
pub fn human_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

/// Parse a duration string (`24h`, `7d`, `30m`, `1mo`, `1y`) or ISO-8601
/// timestamp into an epoch-seconds cutoff.
pub fn parse_since(raw: &str) -> Result<i64, String> {
    if let Ok(dt) = raw.parse::<chrono::DateTime<chrono::Utc>>() {
        return Ok(dt.timestamp());
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.timestamp());
    }
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err("expected number + suffix (s/m/h/d/w/mo/y) or ISO-8601".into());
    }
    let n: i64 = digits
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    let suffix = &raw[digits.len()..];
    let seconds: i64 = match suffix {
        "s" => Some(n),
        "m" => n.checked_mul(60),
        "h" => n.checked_mul(3600),
        "d" => n.checked_mul(86400),
        "w" => n.checked_mul(7).and_then(|v| v.checked_mul(86400)),
        "mo" => n.checked_mul(30).and_then(|v| v.checked_mul(86400)),
        "y" => n.checked_mul(365).and_then(|v| v.checked_mul(86400)),
        other => return Err(format!("unknown suffix '{other}'")),
    }
    .ok_or_else(|| "duration is too large".to_string())?;
    Ok(chrono::Utc::now().timestamp() - seconds)
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
