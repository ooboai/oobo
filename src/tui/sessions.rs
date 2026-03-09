use std::collections::HashMap;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::db::stats::StatsRow;
use crate::db::Db;
use crate::session;
use crate::tools::cursor::Session;

#[allow(dead_code)]
struct SessionRow {
    session: Session,
    msg_count: u32,
    stats: Option<StatsRow>,
}

enum View {
    List,
    Search,
    Show {
        idx: usize,
        scroll: u16,
        lines: Vec<Line<'static>>,
    },
}

pub fn run_list(sessions: Vec<Session>, show_all: bool) -> Result<(), String> {
    let stats_map = load_stats_map(&sessions);

    let rows: Vec<SessionRow> = sessions
        .into_iter()
        .map(|s| {
            let msg_count = session::count_messages(&s);
            let key = (s.session_id.clone(), s.source.clone());
            let stats = stats_map.get(&key).cloned();
            SessionRow {
                session: s,
                msg_count,
                stats,
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
    let lines = build_show_lines(&session, None);
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
            .draw(|f| render_show(f, s, l, sc, None))
            .map_err(|e| e.to_string())?;

        if let Some(code) = crate::tui::key_code(crate::tui::KEY_POLL).map_err(|e| e.to_string())? {
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

fn load_stats_map(sessions: &[Session]) -> HashMap<(String, String), StatsRow> {
    let db = match Db::open() {
        Ok(db) => db,
        Err(_) => return HashMap::new(),
    };
    let keys: Vec<(String, String)> = sessions
        .iter()
        .map(|s| (s.session_id.clone(), s.source.clone()))
        .collect();
    db.get_stats_bulk(&keys).unwrap_or_default()
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    rows: &[SessionRow],
    table_state: &mut TableState,
    view: &mut View,
    show_all: bool,
) -> Result<(), String> {
    let mut search_query = String::new();
    let mut filtered_indices: Vec<usize> = (0..rows.len()).collect();

    loop {
        match view {
            View::List => {
                let display_rows: Vec<&SessionRow> =
                    filtered_indices.iter().map(|&i| &rows[i]).collect();
                let filter_text = if search_query.is_empty() {
                    None
                } else {
                    Some(search_query.as_str())
                };
                terminal
                    .draw(|f| render_list(f, &display_rows, table_state, show_all, filter_text))
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(crate::tui::KEY_POLL).map_err(|e| e.to_string())?
                {
                    let len = filtered_indices.len();
                    match code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            if !search_query.is_empty() {
                                search_query.clear();
                                filtered_indices = (0..rows.len()).collect();
                                table_state.select(Some(0));
                            } else {
                                return Ok(());
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if len > 0 {
                                let i = table_state.selected().map_or(0, |i| {
                                    if i + 1 >= len {
                                        0
                                    } else {
                                        i + 1
                                    }
                                });
                                table_state.select(Some(i));
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if len > 0 {
                                let i = table_state.selected().map_or(0, |i| {
                                    if i == 0 {
                                        len.saturating_sub(1)
                                    } else {
                                        i - 1
                                    }
                                });
                                table_state.select(Some(i));
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(sel) = table_state.selected() {
                                if sel < filtered_indices.len() {
                                    let real_idx = filtered_indices[sel];
                                    let lines = build_show_lines(
                                        &rows[real_idx].session,
                                        rows[real_idx].stats.as_ref(),
                                    );
                                    *view = View::Show {
                                        idx: real_idx,
                                        scroll: 0,
                                        lines,
                                    };
                                }
                            }
                        }
                        KeyCode::Char('/') => {
                            *view = View::Search;
                        }
                        _ => {}
                    }
                }
            }
            View::Search => {
                let display_rows: Vec<&SessionRow> =
                    filtered_indices.iter().map(|&i| &rows[i]).collect();
                let filter_text = Some(search_query.as_str());
                terminal
                    .draw(|f| {
                        render_list(f, &display_rows, table_state, show_all, filter_text);
                        let area = f.area();
                        let search_area = ratatui::layout::Rect {
                            x: 0,
                            y: area.height.saturating_sub(1),
                            width: area.width,
                            height: 1,
                        };
                        let search_line = Line::from(vec![
                            Span::styled(
                                "/",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(search_query.clone(), Style::default().fg(Color::White)),
                            Span::styled("▌", Style::default().fg(Color::White)),
                        ]);
                        f.render_widget(Paragraph::new(search_line), search_area);
                    })
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(crate::tui::KEY_POLL).map_err(|e| e.to_string())?
                {
                    match code {
                        KeyCode::Esc => {
                            *view = View::List;
                        }
                        KeyCode::Enter => {
                            *view = View::List;
                        }
                        KeyCode::Backspace => {
                            search_query.pop();
                            filtered_indices = apply_filter(rows, &search_query);
                            table_state.select(Some(0));
                        }
                        KeyCode::Char(c) => {
                            search_query.push(c);
                            filtered_indices = apply_filter(rows, &search_query);
                            table_state.select(Some(0));
                        }
                        _ => {}
                    }
                }
            }
            View::Show { idx, scroll, lines } => {
                let session = &rows[*idx].session;
                let stats = rows[*idx].stats.as_ref();
                let sc = *scroll;
                terminal
                    .draw(|f| render_show(f, session, lines, sc, stats))
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(crate::tui::KEY_POLL).map_err(|e| e.to_string())?
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

fn apply_filter(rows: &[SessionRow], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..rows.len()).collect();
    }
    let q = query.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| {
            let name_match = r.session.name.to_lowercase().contains(&q);
            let source_match = r.session.source.to_lowercase().contains(&q)
                || crate::tui::source_label(&r.session.source)
                    .to_lowercase()
                    .contains(&q);
            let model_match = r
                .stats
                .as_ref()
                .and_then(|s| s.model.as_deref())
                .is_some_and(|m| m.to_lowercase().contains(&q));
            let project_match = r.session.project_path.to_lowercase().contains(&q);
            name_match || source_match || model_match || project_match
        })
        .map(|(i, _)| i)
        .collect()
}

fn render_list(
    f: &mut Frame,
    rows: &[&SessionRow],
    state: &mut TableState,
    show_all: bool,
    filter: Option<&str>,
) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let count_str = format!("({} sessions) ", rows.len());
    let mut title_spans = vec![
        Span::styled(
            " oobo sessions ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if show_all { "• all projects " } else { "" },
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(count_str, Style::default().fg(Color::DarkGray)),
    ];
    if let Some(q) = filter {
        if !q.is_empty() {
            title_spans.push(Span::styled(
                format!(" filter: \"{q}\""),
                Style::default().fg(Color::Yellow),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(title_spans)), chunks[0]);

    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("Src"),
        Cell::from("Updated"),
        Cell::from("Model"),
        Cell::from("Tokens"),
        Cell::from("Dur"),
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
            } else if s.created_at.is_some() {
                s.created_at_iso()
            } else {
                "—".into()
            };

            let model = r
                .stats
                .as_ref()
                .and_then(|st| st.model.as_deref())
                .map(shorten_model)
                .unwrap_or_else(|| "—".into());

            let tokens = r
                .stats
                .as_ref()
                .map(|st| {
                    let inp = st.input_tokens.unwrap_or(0);
                    let out = st.output_tokens.unwrap_or(0);
                    let total = inp + out;
                    if total == 0 {
                        "—".into()
                    } else if st.is_estimated {
                        format!("~{}", format_tokens(total))
                    } else {
                        format_tokens(total)
                    }
                })
                .unwrap_or_else(|| "—".into());

            let dur = r
                .stats
                .as_ref()
                .and_then(|st| st.duration_secs)
                .map(format_duration)
                .unwrap_or_else(|| "—".into());

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
            let src = source_label(&s.source);

            let short_id = s.short_id();

            Row::new(vec![
                Cell::from(short_id).style(Style::default().fg(Color::DarkGray)),
                Cell::from(src),
                Cell::from(updated),
                Cell::from(model),
                Cell::from(tokens).style(Style::default().fg(Color::Yellow)),
                Cell::from(dur),
                Cell::from(name),
            ])
        })
        .collect();

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(9),
            Constraint::Length(7),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(7),
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
            "/",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" search  ", Style::default().fg(Color::DarkGray)),
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

fn render_show(
    f: &mut Frame,
    session: &Session,
    lines: &[Line<'static>],
    scroll: u16,
    stats: Option<&StatsRow>,
) {
    let area = f.area();

    let header_height = if stats.is_some() { 7 } else { 4 };
    let chunks = Layout::vertical([
        Constraint::Length(header_height),
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

    let mut header_lines = vec![
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

    if let Some(st) = stats {
        let model_str = st.model.as_deref().unwrap_or("unknown");
        let inp = st.input_tokens.unwrap_or(0);
        let out = st.output_tokens.unwrap_or(0);
        let dur = st.duration_secs.map(format_duration).unwrap_or_default();
        let files_count = st.files_touched.len();
        let tools = st.tool_call_count;

        header_lines.push(Line::from(""));
        let stat_spans = vec![
            Span::styled(" Model: ", Style::default().fg(Color::DarkGray)),
            Span::styled(model_str.to_string(), Style::default().fg(Color::Magenta)),
            Span::styled("  Tokens: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if st.is_estimated {
                    format!("~{}in / ~{}out", format_tokens(inp), format_tokens(out))
                } else {
                    format!("{}in / {}out", format_tokens(inp), format_tokens(out))
                },
                Style::default().fg(Color::Yellow),
            ),
        ];
        header_lines.push(Line::from(stat_spans));

        let mut detail_spans = vec![];
        if !dur.is_empty() {
            detail_spans.push(Span::styled(
                " Duration: ",
                Style::default().fg(Color::DarkGray),
            ));
            detail_spans.push(Span::styled(dur, Style::default().fg(Color::White)));
        }
        if tools > 0 {
            detail_spans.push(Span::styled(
                "  Tool calls: ",
                Style::default().fg(Color::DarkGray),
            ));
            detail_spans.push(Span::styled(
                tools.to_string(),
                Style::default().fg(Color::White),
            ));
        }
        if files_count > 0 {
            detail_spans.push(Span::styled(
                "  Files: ",
                Style::default().fg(Color::DarkGray),
            ));
            detail_spans.push(Span::styled(
                files_count.to_string(),
                Style::default().fg(Color::White),
            ));
        }
        if !detail_spans.is_empty() {
            header_lines.push(Line::from(detail_spans));
        }
    }

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

fn build_show_lines(session: &Session, stats: Option<&StatsRow>) -> Vec<Line<'static>> {
    let path = session::find_transcript_path(session);
    let mut messages = path
        .as_ref()
        .map(|p| session::parse_messages(p, &session.source))
        .unwrap_or_default();

    if messages.is_empty() {
        messages = session::parse_messages_for_session(
            &session.project_path,
            &session.session_id,
            &session.source,
        );
    }

    // Fall back to bubble data for Cursor sessions with no file-based transcript
    if messages.is_empty() && session.source == "composer" {
        let ids = vec![session.session_id.clone()];
        let bubble_map = crate::tools::cursor::composer_data::preload_bubble_data_for(&ids);
        if let Some(bs) = bubble_map.get(&session.session_id) {
            messages = bs.messages.clone();
        }
        if messages.is_empty() {
            let composer_map = crate::tools::cursor::composer_data::preload_composer_data_for(&ids);
            if let Some(cs) = composer_map.get(&session.session_id) {
                messages = cs.messages.clone();
            }
        }
    }

    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(st) = stats {
        if !st.files_touched.is_empty() {
            lines.push(Line::from(Span::styled(
                "━━ FILES TOUCHED ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
                Style::default().fg(Color::Magenta),
            )));
            for f in &st.files_touched {
                let short = std::path::Path::new(f)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(f);
                lines.push(Line::from(format!("  {short}")));
            }
            lines.push(Line::from(""));
        }
    }

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

fn shorten_model(model: &str) -> String {
    let m = model
        .replace("claude-", "")
        .replace("gpt-", "gpt")
        .replace("-preview", "")
        .replace("-20251101", "")
        .replace("-20250101", "")
        .replace("-20260101", "");
    if m.len() > 14 {
        format!("{}…", &m[..13])
    } else {
        m
    }
}

fn format_tokens(n: i64) -> String {
    crate::tui::format_tokens(n)
}

fn format_duration(secs: i64) -> String {
    crate::tui::format_duration(secs)
}

fn source_label(source: &str) -> String {
    crate::tui::source_label(source).to_string()
}
