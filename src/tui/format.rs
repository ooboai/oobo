use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

#[allow(dead_code)]
pub(super) fn display_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

#[allow(dead_code)]
pub(super) fn short_agent_label(source: &str) -> &str {
    let s = source.to_lowercase();
    if s.contains("claude") {
        "claude"
    } else if s.contains("cursor") {
        "cursor"
    } else if s.contains("codex") {
        "codex"
    } else if s.contains("copilot") {
        "copilot"
    } else if s.contains("gemini") {
        "gemini"
    } else if s.contains("aider") {
        "aider"
    } else {
        // Best-effort: show first 8 chars of whatever source label is.
        source
    }
}

/// Check if a line starts with "1. ", "2. ", etc. and return the rest.
pub(super) fn strip_numbered_prefix(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() && bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1] == b' ' {
        return Some(&s[i + 2..]);
    }
    None
}

pub(super) fn role_label(role: &str) -> &'static str {
    match role {
        "user" => "You",
        "assistant" => "AI",
        "system" => "System",
        "tool" => "Tool",
        _ => "?",
    }
}

pub(super) fn role_style(role: &str) -> Style {
    match role {
        "user" => Style::default().fg(Color::Cyan),
        "assistant" => Style::default().fg(Color::Green),
        "system" => Style::default().fg(Color::DarkGray),
        "tool" => Style::default().fg(Color::Magenta),
        _ => Style::default(),
    }
}

pub(super) fn timestamp_short(ts_ms: Option<i64>) -> String {
    match ts_ms {
        Some(ms) => chrono::DateTime::from_timestamp(ms / 1000, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

pub(super) fn pad_or_truncate(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count > width {
        let keep = width.saturating_sub(1);
        let mut out: String = s.chars().take(keep).collect();
        out.push('…');
        out
    } else {
        let mut out = s.to_string();
        for _ in 0..(width - count) {
            out.push(' ');
        }
        out
    }
}

pub(super) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

pub(super) fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

pub(super) fn relative_time(ts: i64) -> String {
    crate::utils::relative_time(ts)
}

pub(super) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        format!("{s:<max$}")
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
