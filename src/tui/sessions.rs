use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::cursor::Session;
use crate::session;

struct SessionRow {
    session: Session,
    msg_count: u32,
}

enum View {
    List,
    Show {
        idx: usize,
        scroll: u16,
        lines: Vec<Line<'static>>,
    },
}

pub fn run_list(sessions: Vec<Session>, show_all: bool) -> Result<(), String> {
    let rows: Vec<SessionRow> = sessions
        .into_iter()
        .map(|s| {
            let msg_count = session::count_messages(&s);
            SessionRow {
                session: s,
                msg_count,
            }
        })
        .collect();

    if rows.is_empty() {
        eprintln!("No sessions found.");
        return Ok(());
    }

    let mut terminal = crate::tui::init().map_err(|e| e.to_string())?;
    let mut table_state = TableState::default();
    table_state.select(Some(0));
    let mut view = View::List;

    let result = run_loop(&mut terminal, &rows, &mut table_state, &mut view, show_all);
    crate::tui::restore();
    result
}

pub fn run_show(session: Session) -> Result<(), String> {
    let lines = build_show_lines(&session);
    if lines.is_empty() {
        eprintln!("No transcript found for session {}", session.short_id());
        return Ok(());
    }

    let mut terminal = crate::tui::init().map_err(|e| e.to_string())?;
    let mut scroll: u16 = 0;

    loop {
        let s = &session;
        let l = &lines;
        let sc = scroll;
        terminal
            .draw(|f| render_show(f, s, l, sc))
            .map_err(|e| e.to_string())?;

        if let Some(code) =
            crate::tui::key_code(Duration::from_millis(50)).map_err(|e| e.to_string())?
        {
            match code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => scroll = scroll.saturating_add(20),
                KeyCode::PageUp => scroll = scroll.saturating_sub(20),
                KeyCode::Home | KeyCode::Char('g') => scroll = 0,
                KeyCode::End | KeyCode::Char('G') => {
                    scroll = (lines.len() as u16).saturating_sub(5);
                }
                _ => {}
            }
        }
    }

    crate::tui::restore();
    Ok(())
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    rows: &[SessionRow],
    table_state: &mut TableState,
    view: &mut View,
    show_all: bool,
) -> Result<(), String> {
    loop {
        match view {
            View::List => {
                terminal
                    .draw(|f| render_list(f, rows, table_state, show_all))
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(Duration::from_millis(50)).map_err(|e| e.to_string())?
                {
                    match code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Down | KeyCode::Char('j') => {
                            let i = table_state.selected().map_or(0, |i| {
                                if i + 1 >= rows.len() {
                                    0
                                } else {
                                    i + 1
                                }
                            });
                            table_state.select(Some(i));
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            let i = table_state.selected().map_or(0, |i| {
                                if i == 0 {
                                    rows.len() - 1
                                } else {
                                    i - 1
                                }
                            });
                            table_state.select(Some(i));
                        }
                        KeyCode::Enter => {
                            if let Some(idx) = table_state.selected() {
                                let lines = build_show_lines(&rows[idx].session);
                                *view = View::Show {
                                    idx,
                                    scroll: 0,
                                    lines,
                                };
                            }
                        }
                        _ => {}
                    }
                }
            }
            View::Show { idx, scroll, lines } => {
                let session = &rows[*idx].session;
                let sc = *scroll;
                terminal
                    .draw(|f| render_show(f, session, lines, sc))
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(Duration::from_millis(50)).map_err(|e| e.to_string())?
                {
                    match code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            *view = View::List;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            *scroll = scroll.saturating_add(1);
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            *scroll = scroll.saturating_sub(1);
                        }
                        KeyCode::PageDown | KeyCode::Char(' ') => {
                            *scroll = scroll.saturating_add(20);
                        }
                        KeyCode::PageUp => {
                            *scroll = scroll.saturating_sub(20);
                        }
                        KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                        KeyCode::End | KeyCode::Char('G') => {
                            *scroll = (lines.len() as u16).saturating_sub(5);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn render_list(f: &mut Frame, rows: &[SessionRow], state: &mut TableState, show_all: bool) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let title = Line::from(vec![
        Span::styled(
            " oobo sessions ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if show_all { "• all projects" } else { "" },
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(title), chunks[0]);

    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("Src"),
        Cell::from("Updated"),
        Cell::from("Msgs"),
        Cell::from("Mode"),
        Cell::from("Title"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            let s = &r.session;
            let updated = if s.updated_at.is_some() {
                s.updated_at_iso()
            } else {
                "—".into()
            };
            let msgs = if r.msg_count > 0 {
                r.msg_count.to_string()
            } else {
                "—".into()
            };
            let name = if s.name.is_empty() {
                "(untitled)".to_string()
            } else if show_all {
                let proj = std::path::Path::new(&s.project_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                format!("{}  [{}]", s.name, proj)
            } else {
                s.name.clone()
            };
            let mode: String = s.mode.chars().take(8).collect();
            let src = source_label(&s.source);

            Row::new(vec![
                Cell::from(s.short_id().to_string()),
                Cell::from(src),
                Cell::from(updated),
                Cell::from(msgs),
                Cell::from(mode),
                Cell::from(name),
            ])
        })
        .collect();

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Length(20),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::bordered())
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(table, chunks[1], state);

    let footer = Line::from(vec![
        Span::styled(
            " ↑↓",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "enter",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" open  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

fn render_show(f: &mut Frame, session: &Session, lines: &[Line<'static>], scroll: u16) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let title_text = if session.name.is_empty() {
        "(untitled)"
    } else {
        &session.name
    };
    let src = source_label(&session.source);
    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                " Session ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(session.short_id(), Style::default().fg(Color::White)),
            Span::styled(format!("  [{src}]"), Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![Span::styled(
            format!(" {title_text}"),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!(
                " {}  •  {}  •  {}",
                session.mode,
                session.created_at_iso(),
                session.updated_at_iso()
            ),
            Style::default().fg(Color::DarkGray),
        )]),
    ];
    f.render_widget(
        Paragraph::new(header_lines).block(Block::bordered()),
        chunks[0],
    );

    let body = Paragraph::new(lines.to_vec())
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .block(Block::bordered());
    f.render_widget(body, chunks[1]);

    let scroll_pct = if lines.len() > 1 {
        let max = lines.len().saturating_sub(1) as f64;
        let pct = (scroll as f64 / max * 100.0).min(100.0) as u16;
        format!(" {pct}%")
    } else {
        String::new()
    };

    let footer = Line::from(vec![
        Span::styled(
            " ↑↓/j/k",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "pgup/pgdn",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" page  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" back", Style::default().fg(Color::DarkGray)),
        Span::styled(scroll_pct, Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

fn build_show_lines(session: &Session) -> Vec<Line<'static>> {
    let path = session::find_transcript_path(session);
    let messages = path
        .as_ref()
        .map(|p| session::parse_messages(p, &session.source))
        .unwrap_or_default();

    let mut lines: Vec<Line<'static>> = Vec::new();

    for msg in &messages {
        let (role_color, role_label) = if msg.role == "user" {
            (Color::Yellow, "USER")
        } else {
            (Color::Green, "ASSISTANT")
        };

        let sep = format!("━━ {role_label} ");
        let pad = 60usize.saturating_sub(sep.len());
        let full = format!("{sep}{}", "━".repeat(pad));
        lines.push(Line::from(Span::styled(
            full,
            Style::default().fg(role_color),
        )));

        for text_line in msg.text.lines() {
            lines.push(Line::from(format!("  {text_line}")));
        }

        lines.push(Line::from(""));
    }

    if messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no transcript found)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}

fn source_label(source: &str) -> String {
    match source {
        "claude" => "claude".to_string(),
        "composer" => "cursor".to_string(),
        _ => source.to_string(),
    }
}
