pub mod app;
pub mod format;
pub mod setup;
pub mod transcript;
mod types;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;

pub const KEY_POLL: Duration = Duration::from_millis(50);

pub fn init() -> io::Result<DefaultTerminal> {
    ratatui::try_init()
}

pub fn restore() {
    ratatui::restore();
}

pub fn next_key(timeout: Duration) -> io::Result<Option<KeyEvent>> {
    if event::poll(timeout)? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                return Ok(Some(key));
            }
        }
    }
    Ok(None)
}

#[allow(dead_code)]
pub fn key_code(timeout: Duration) -> io::Result<Option<KeyCode>> {
    Ok(next_key(timeout)?.map(|k| k.code))
}

/// Map internal source identifiers to human-readable tool names.
#[allow(dead_code)]
pub fn source_label(source: &str) -> &'static str {
    match source {
        "composer" => "Cursor",
        "cursor" => "Cursor",
        "claude" => "Claude",
        "windsurf" => "Windsurf",
        "trae" => "Trae",
        "aider" => "Aider",
        "copilot" => "Copilot",
        "zed" => "Zed",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "gemini" => "Gemini CLI",
        _ => "Unknown",
    }
}

pub fn format_tokens(n: i64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[allow(dead_code)]
pub fn format_duration(secs: i64) -> String {
    if secs >= 86400 {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        format!("{days}d {hours}h")
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[allow(dead_code)]
pub fn kv_line(label: &str, value: &str) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    Line::from(vec![
        Span::styled(
            format!("  {label:<14} "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(value.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(999_999), "1000.0K");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn test_format_tokens_billions() {
        assert_eq!(format_tokens(1_000_000_000), "1.0B");
        assert_eq!(format_tokens(3_700_000_000), "3.7B");
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(59), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(60), "1m 0s");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3599), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600), "1h 0m");
        assert_eq!(format_duration(7200), "2h 0m");
        assert_eq!(format_duration(5400), "1h 30m");
        assert_eq!(format_duration(86399), "23h 59m");
    }

    #[test]
    fn test_format_duration_days() {
        assert_eq!(format_duration(86400), "1d 0h");
        assert_eq!(format_duration(172800), "2d 0h");
        assert_eq!(format_duration(90000), "1d 1h");
    }

    #[test]
    fn test_source_label_known() {
        assert_eq!(source_label("composer"), "Cursor");
        assert_eq!(source_label("cursor"), "Cursor");
        assert_eq!(source_label("claude"), "Claude");
        assert_eq!(source_label("gemini"), "Gemini CLI");
        assert_eq!(source_label("aider"), "Aider");
        assert_eq!(source_label("codex"), "Codex");
        assert_eq!(source_label("opencode"), "OpenCode");
    }

    #[test]
    fn test_source_label_unknown() {
        assert_eq!(source_label("unknown-tool"), "Unknown");
        assert_eq!(source_label(""), "Unknown");
    }
}
