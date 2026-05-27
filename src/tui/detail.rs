use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use super::app::App;
use super::data::{load_sessions_for_anchor, touched_files_for};
use super::draw::chip;
use super::format::{centered, relative_time, short_sha};
use super::types::{AnchorRow, MemoryKind, PickerState};

// ── Anchor detail ─────────────────────────────────────────────────────

pub(super) fn draw_anchor_detail(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let Some(anchor) = app.selected_anchor() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  (no anchor selected)",
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    };

    if anchor.kind == MemoryKind::ShadowAnchor {
        draw_shadow_detail(frame, app, &anchor, area);
        return;
    }

    let sessions = load_sessions_for_anchor(&app.root, &anchor.sha);
    let files = touched_files_for(&app.root, &anchor.sha);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("  ● ", Style::default().fg(Color::Green)),
        Span::styled(
            format!("  {}", short_sha(&anchor.sha)),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} ago", relative_time(anchor.timestamp)),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "  committed memory",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", anchor.subject),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if let Some(intent) = anchor.intent.as_deref() {
        if !intent.is_empty() && intent != anchor.subject {
            lines.push(Line::from(Span::styled(
                format!("  {intent}"),
                Style::default().fg(Color::Gray),
            )));
        }
    }
    lines.push(Line::from(""));

    let tool = anchor.tool.as_deref().unwrap_or("-");
    let tokens = anchor
        .tokens
        .map_or_else(|| "-".into(), super::format_tokens);
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        chip("tool", tool, Color::Cyan),
        Span::styled("  ", Style::default()),
        chip("tokens", &tokens, Color::Green),
        Span::styled("  ", Style::default()),
        chip(
            "sessions",
            &anchor.session_count.to_string(),
            Color::Magenta,
        ),
    ]));

    if !sessions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  sessions",
            Style::default().fg(Color::DarkGray),
        )));
        for s in &sessions {
            let short_sid: String = s.session_id.chars().take(8).collect();
            let model = s.model.clone().unwrap_or_else(|| "-".into());
            let tok = if s.tokens > 0 {
                super::format_tokens(s.tokens)
            } else {
                "-".into()
            };
            lines.push(Line::from(vec![
                Span::styled("    · ", Style::default().fg(Color::Green)),
                Span::raw(short_sid),
                Span::styled(
                    format!("  {}  {}  {tok}", s.source, model),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    if !files.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  files ({})", files.len()),
            Style::default().fg(Color::DarkGray),
        )));
        for f in files.iter().take(12) {
            lines.push(Line::from(Span::raw(format!("    {f}"))));
        }
        if files.len() > 12 {
            lines.push(Line::from(Span::styled(
                format!("    … {} more", files.len() - 12),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  actions",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(vec![
        Span::styled("    enter", Style::default().fg(Color::White)),
        Span::styled(" memory", Style::default().fg(Color::DarkGray)),
        Span::styled("   d", Style::default().fg(Color::White)),
        Span::styled(" diff", Style::default().fg(Color::DarkGray)),
        Span::styled("   b", Style::default().fg(Color::White)),
        Span::styled(" blame", Style::default().fg(Color::DarkGray)),
    ]));

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn draw_shadow_detail(frame: &mut ratatui::Frame<'_>, app: &App, shadow: &AnchorRow, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("  · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("  {}", short_sha(&shadow.sha)),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} ago", relative_time(shadow.timestamp)),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "  working memory",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", shadow.subject),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        chip(
            "source",
            &shadow.tool.clone().unwrap_or_else(|| "-".to_string()),
            Color::Cyan,
        ),
        Span::styled("  ", Style::default()),
        chip("files", &shadow.files.to_string(), Color::Gray),
        Span::styled("  ", Style::default()),
        chip("tools", &shadow.tool_calls.to_string(), Color::Magenta),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  session ", Style::default().fg(Color::DarkGray)),
        Span::raw(
            shadow
                .session_id
                .as_deref()
                .map_or_else(|| "-".to_string(), short_session),
        ),
    ]));
    if let Some(parent) = shadow.parent_anchor.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("  follows ", Style::default().fg(Color::DarkGray)),
            Span::raw(short_sha(parent)),
        ]));
    }
    if let Some(wt) = &shadow.worktree_hint {
        lines.push(Line::from(vec![
            Span::styled("  worktree ", Style::default().fg(Color::DarkGray)),
            Span::styled(wt.clone(), Style::default().fg(Color::Cyan)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  continue from here",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(vec![
        Span::styled("    oobo goto ", Style::default().fg(Color::DarkGray)),
        Span::styled(shadow.sha.clone(), Style::default().fg(Color::Gray)),
    ]));
    lines.push(Line::from(""));
    if app.dirty {
        lines.push(Line::from(Span::styled(
            "  worktree is dirty; loading this point will require --force",
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled("  enter", Style::default().fg(Color::White)),
        Span::styled(
            " opens captured memory",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("   c", Style::default().fg(Color::White)),
        Span::styled(" continues from here", Style::default().fg(Color::DarkGray)),
    ]));

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn short_session(session_id: &str) -> String {
    session_id.chars().take(18).collect()
}

// ── Search view ───────────────────────────────────────────────────────

// ── Picker overlay ────────────────────────────────────────────────────

pub(super) fn draw_picker_overlay(frame: &mut ratatui::Frame<'_>, p: &PickerState) {
    let area = frame.area();
    let height = (p.len() as u16 + 4).clamp(6, 20);
    let rect = centered(area, 70, height);
    frame.render_widget(Clear, rect);

    let items: Vec<ListItem> = (0..p.len())
        .map(|i| ListItem::new(Line::from(Span::raw(p.row_label(i)))))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", p.title))
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut ls = p.list;
    frame.render_stateful_widget(list, rect, &mut ls);
}

// ── Help overlay ──────────────────────────────────────────────────────

pub(super) fn draw_help_overlay(frame: &mut ratatui::Frame<'_>) {
    let area = frame.area();
    let rect = centered(area, 62, 27);

    let lines = vec![
        Line::from(Span::styled(
            " oobo TUI · keybindings",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(" FEED"),
        Line::from("   ↑/k ↓/j     move selection"),
        Line::from("   g / G       jump to top / bottom"),
        Line::from("   enter       open transcript (session picker if >1)"),
        Line::from("   d           view commit diff inline"),
        Line::from("   b           blame (file picker)"),
        Line::from("   L           goto selected (time-travel, auto-stashes)"),
        Line::from("   /           live filter (type to narrow list)"),
        Line::from("   s           remote recall (sessions + anchors)"),
        Line::from("   t           cycle time window (all / 24h / 7d / 30d)"),
        Line::from("   e           toggle tracking on/off"),
        Line::from("   r           reload"),
        Line::from(""),
        Line::from(" TRANSCRIPT / DIFF"),
        Line::from("   ↑↓ pgup/pgdn  scroll · g/G top/bot"),
        Line::from("   [ / ]         prev / next session"),
        Line::from("   /             search in transcript · n/N next/prev"),
        Line::from("   t / T         next / prev tool call"),
        Line::from("   esc           back · q quit"),
        Line::from(""),
        Line::from(" FILTER (when active)"),
        Line::from("   type        filter the list live"),
        Line::from("   enter       confirm · esc clear · ctrl-c clear"),
        Line::from(""),
        Line::from(Span::styled(
            " press any key to dismiss",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    frame.render_widget(Clear, rect);
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" help ")
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(para, rect);
}
