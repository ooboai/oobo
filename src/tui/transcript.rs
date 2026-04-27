use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::format::{role_label, role_style, strip_numbered_prefix, timestamp_short};
use super::types::{SessionLink, TranscriptState};

pub(super) fn load_transcript_lines(project_root: &str, s: &SessionLink) -> Vec<Line<'static>> {
    let messages =
        crate::session::parse_messages_for_session(project_root, &s.session_id, &s.source);
    if messages.is_empty() {
        return vec![
            Line::from(""),
            Line::from(Span::styled(
                "  (transcript not available for this session)",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  session {} · {}", &s.session_id, s.source),
                Style::default().fg(Color::DarkGray),
            )),
        ];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for m in &messages {
        render_message(&mut out, m);
    }
    out
}

pub(super) fn render_message(out: &mut Vec<Line<'static>>, m: &crate::core::message::Message) {
    let role = role_label(&m.role);
    let r_style = role_style(&m.role);

    let ts = timestamp_short(m.timestamp_ms);
    let ts_part = if ts.is_empty() {
        String::new()
    } else {
        format!("  {ts}")
    };

    let separator: String = "─".repeat(60);
    out.push(Line::from(Span::styled(
        format!("  {separator}"),
        Style::default().fg(Color::DarkGray),
    )));
    out.push(Line::from(vec![
        Span::styled(format!("  {role}"), r_style.add_modifier(Modifier::BOLD)),
        Span::styled(ts_part, Style::default().fg(Color::DarkGray)),
    ]));
    out.push(Line::from(""));

    let text = &m.text;
    let body_style = match m.role.as_str() {
        "tool" => Style::default().fg(Color::DarkGray),
        _ => Style::default(),
    };

    let mut in_code_block = false;
    let mut code_lang = String::new();

    for raw_line in text.lines() {
        if raw_line.trim_start().starts_with("```") {
            if in_code_block {
                out.push(Line::from(Span::styled(
                    "  └───",
                    Style::default().fg(Color::DarkGray),
                )));
                in_code_block = false;
                code_lang.clear();
                continue;
            } else {
                code_lang = raw_line
                    .trim_start()
                    .trim_start_matches('`')
                    .trim()
                    .to_string();
                let label = if code_lang.is_empty() {
                    "  ┌─── code".to_string()
                } else {
                    format!("  ┌─── {code_lang}")
                };
                out.push(Line::from(Span::styled(
                    label,
                    Style::default().fg(Color::DarkGray),
                )));
                in_code_block = true;
                continue;
            }
        }

        if in_code_block {
            out.push(Line::from(vec![
                Span::styled("  │ ", Style::default().fg(Color::DarkGray)),
                Span::styled(raw_line.to_string(), Style::default().fg(Color::Yellow)),
            ]));
            continue;
        }

        let trimmed = raw_line.trim();

        if trimmed.is_empty() {
            out.push(Line::from(""));
            continue;
        }

        if let Some(heading) = trimmed.strip_prefix("### ") {
            out.push(Line::from(Span::styled(
                format!("  {heading}"),
                body_style
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            out.push(Line::from(Span::styled(
                format!("  {heading}"),
                body_style
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("# ") {
            out.push(Line::from(Span::styled(
                format!("  {heading}"),
                body_style
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            continue;
        }

        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let indent = raw_line.len() - raw_line.trim_start().len();
            let pad = " ".repeat(indent + 2);
            out.push(Line::from(vec![
                Span::raw(pad),
                Span::styled("• ", body_style.fg(Color::Cyan)),
                Span::styled(trimmed[2..].to_string(), body_style),
            ]));
            continue;
        }

        if let Some(rest) = strip_numbered_prefix(trimmed) {
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{}. ", trimmed.split('.').next().unwrap_or("1")),
                    body_style.fg(Color::Cyan),
                ),
                Span::styled(rest.to_string(), body_style),
            ]));
            continue;
        }

        out.push(render_inline_markdown(raw_line, body_style));
    }

    if in_code_block {
        out.push(Line::from(Span::styled(
            "  └───",
            Style::default().fg(Color::DarkGray),
        )));
    }

    out.push(Line::from(""));
}

/// Render a line of text with inline `code`, **bold**, and *italic* spans.
pub(super) fn render_inline_markdown(line: &str, base: Style) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::raw("  ")); // left margin

    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();

    while i < len {
        if chars[i] == '`' && !matches!(chars.get(i + 1), Some('`')) {
            if !buf.is_empty() {
                spans.push(Span::styled(buf.clone(), base));
                buf.clear();
            }
            i += 1;
            let mut code = String::new();
            while i < len && chars[i] != '`' {
                code.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1;
            }
            spans.push(Span::styled(code, Style::default().fg(Color::Yellow)));
            continue;
        }

        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') && i + 2 < len && chars[i + 2] != '*' {
            if !buf.is_empty() {
                spans.push(Span::styled(buf.clone(), base));
                buf.clear();
            }
            i += 2;
            let mut bold = String::new();
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                bold.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                i += 2;
            }
            spans.push(Span::styled(bold, base.add_modifier(Modifier::BOLD)));
            continue;
        }

        buf.push(chars[i]);
        i += 1;
    }

    if !buf.is_empty() {
        spans.push(Span::styled(buf, base));
    }

    Line::from(spans)
}

pub(super) fn draw_transcript(frame: &mut ratatui::Frame<'_>, ts: &TranscriptState) {
    let area = frame.area();

    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Reset)),
        area,
    );

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let session = &ts.sessions[ts.idx];
    let short_sid: String = session.session_id.chars().take(8).collect();
    let model = session.model.clone().unwrap_or_default();
    let position = if ts.sessions.len() > 1 {
        format!("  [{}/{}]", ts.idx + 1, ts.sessions.len())
    } else {
        String::new()
    };
    let matches_str = if ts.match_lines.is_empty() {
        String::new()
    } else {
        format!("  {}/{} matches", ts.match_cursor + 1, ts.match_lines.len())
    };

    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                " transcript",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {short_sid}  {}{model}{position}{matches_str}",
                    session.source
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(Span::styled(
            format!(" {}", "─".repeat(area.width.saturating_sub(2) as usize)),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(header_lines), layout[0]);

    let rendered: Vec<Line<'static>> = ts
        .lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if ts.match_lines.contains(&i) {
                let mut new = line.clone();
                for span in new.spans.iter_mut() {
                    span.style = span.style.bg(Color::Yellow).fg(Color::Black);
                }
                new
            } else {
                line.clone()
            }
        })
        .collect();

    let max_scroll = rendered.len().saturating_sub(layout[1].height as usize) as u16;
    let scroll = ts.scroll.min(max_scroll);
    let body = Paragraph::new(rendered)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(body, layout[1]);

    let total_lines = ts.lines.len();
    let visible_h = layout[1].height as usize;
    let pct = if total_lines <= visible_h {
        100
    } else {
        ((scroll as usize) * 100) / (total_lines.saturating_sub(visible_h)).max(1)
    };

    let footer_content: Line<'static> = if ts.filter_open {
        Line::from(vec![
            Span::styled(
                " /",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(ts.filter.clone()),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::styled(
                "   (enter to jump · esc to clear)",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                format!(" {pct}%"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "  ↑↓ scroll · pgup/pgdn · g/G top/bot · [ ] prev/next · / find · n/N next · esc back · q quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(footer_content), layout[2]);
}
