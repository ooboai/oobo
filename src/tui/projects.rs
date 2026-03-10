use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use super::kv_line;
use crate::db::stats::StatsRow;
use crate::db::Db;
use crate::session;

pub struct ProjectDisplay {
    pub name: String,
    pub path: String,
    pub tools: String,
    pub session_count: i64,
    pub tokens: i64,
    pub sessions: Vec<SessionDisplay>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

pub struct SessionDisplay {
    pub id: String,
    pub source: String,
    pub name: String,
    pub tokens: i64,
    pub updated: String,
}

enum View {
    List,
    Detail {
        idx: usize,
        session_state: TableState,
    },
    Session {
        project_idx: usize,
        session_idx: usize,
        scroll: u16,
        lines: Vec<Line<'static>>,
        stats: Box<Option<StatsRow>>,
    },
}

pub fn run(projects: Vec<ProjectDisplay>) -> Result<(), String> {
    if projects.is_empty() {
        eprintln!("no projects found — run `oobo scan` first");
        return Ok(());
    }

    let mut terminal = crate::tui::init().map_err(|e| e.to_string())?;
    let mut state = TableState::default();
    state.select(Some(0));
    let mut view = View::List;

    let result = run_loop(&mut terminal, &projects, &mut state, &mut view);
    crate::tui::restore();
    result
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    projects: &[ProjectDisplay],
    state: &mut TableState,
    view: &mut View,
) -> Result<(), String> {
    loop {
        match view {
            View::List => {
                terminal
                    .draw(|f| render_list(f, projects, state))
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(crate::tui::KEY_POLL).map_err(|e| e.to_string())?
                {
                    match code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Down | KeyCode::Char('j') => {
                            let i = state.selected().map_or(0, |i| {
                                if i + 1 >= projects.len() {
                                    0
                                } else {
                                    i + 1
                                }
                            });
                            state.select(Some(i));
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            let i = state.selected().map_or(0, |i| {
                                if i == 0 {
                                    projects.len() - 1
                                } else {
                                    i - 1
                                }
                            });
                            state.select(Some(i));
                        }
                        KeyCode::Enter => {
                            if let Some(idx) = state.selected() {
                                let mut ss = TableState::default();
                                if !projects[idx].sessions.is_empty() {
                                    ss.select(Some(0));
                                }
                                *view = View::Detail {
                                    idx,
                                    session_state: ss,
                                };
                            }
                        }
                        _ => {}
                    }
                }
            }
            View::Detail { idx, session_state } => {
                let project = &projects[*idx];
                terminal
                    .draw(|f| render_detail(f, project, session_state))
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(crate::tui::KEY_POLL).map_err(|e| e.to_string())?
                {
                    let session_count = project.sessions.len();
                    match code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            *view = View::List;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if session_count > 0 {
                                let i = session_state.selected().map_or(0, |i| {
                                    if i + 1 >= session_count {
                                        0
                                    } else {
                                        i + 1
                                    }
                                });
                                session_state.select(Some(i));
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if session_count > 0 {
                                let i = session_state.selected().map_or(0, |i| {
                                    if i == 0 {
                                        session_count - 1
                                    } else {
                                        i - 1
                                    }
                                });
                                session_state.select(Some(i));
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(si) = session_state.selected() {
                                let sd = &project.sessions[si];
                                let (lines, stats) = load_session_detail(&sd.id, &sd.source);
                                *view = View::Session {
                                    project_idx: *idx,
                                    session_idx: si,
                                    scroll: 0,
                                    lines,
                                    stats: Box::new(stats),
                                };
                            }
                        }
                        _ => {}
                    }
                }
            }
            View::Session {
                project_idx,
                session_idx,
                scroll,
                lines,
                stats,
            } => {
                let sd = &projects[*project_idx].sessions[*session_idx];
                let sc = *scroll;
                let st = (**stats).as_ref();
                terminal
                    .draw(|f| render_session(f, sd, lines, sc, st))
                    .map_err(|e| e.to_string())?;

                if let Some(code) =
                    crate::tui::key_code(crate::tui::KEY_POLL).map_err(|e| e.to_string())?
                {
                    match code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            let mut ss = TableState::default();
                            ss.select(Some(*session_idx));
                            *view = View::Detail {
                                idx: *project_idx,
                                session_state: ss,
                            };
                        }
                        KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
                        KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                        KeyCode::PageDown | KeyCode::Char(' ') => {
                            *scroll = scroll.saturating_add(20)
                        }
                        KeyCode::PageUp => *scroll = scroll.saturating_sub(20),
                        KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                        KeyCode::End | KeyCode::Char('G') => {
                            *scroll = (lines.len() as u16).saturating_sub(5)
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn load_session_detail(session_id: &str, source: &str) -> (Vec<Line<'static>>, Option<StatsRow>) {
    let stats = Db::open()
        .ok()
        .and_then(|db| db.get_stats(session_id, source).ok().flatten());

    let session = session::find_session_any(session_id).ok();
    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(ref st) = stats {
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

    if let Some(ref s) = session {
        let path = session::find_transcript_path(s);
        let mut messages = path
            .as_ref()
            .map(|p| session::parse_messages(p, &s.source))
            .unwrap_or_default();

        if messages.is_empty() {
            messages =
                session::parse_messages_for_session(&s.project_path, &s.session_id, &s.source);
        }

        if messages.is_empty() && s.source == "composer" {
            let ids = vec![s.session_id.clone()];
            let bubble_map = crate::tools::cursor::composer_data::preload_bubble_data_for(&ids);
            if let Some(bs) = bubble_map.get(&s.session_id) {
                messages = bs.messages.clone();
            }
            if messages.is_empty() {
                let composer_map =
                    crate::tools::cursor::composer_data::preload_composer_data_for(&ids);
                if let Some(cs) = composer_map.get(&s.session_id) {
                    messages = cs.messages.clone();
                }
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
    } else {
        lines.push(Line::from(Span::styled(
            "  (session not found)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    (lines, stats)
}

fn render_list(f: &mut Frame, projects: &[ProjectDisplay], state: &mut TableState) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let title = Line::from(vec![
        Span::styled(
            " oobo projects ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({} projects) ", projects.len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(title), chunks[0]);

    let header = Row::new(vec![
        Cell::from("Project"),
        Cell::from("Sessions"),
        Cell::from("Tools"),
        Cell::from("Tokens"),
        Cell::from("Path"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = projects
        .iter()
        .map(|p| {
            let name = if p.name.len() > 22 {
                format!("{}…", &p.name[..21])
            } else {
                p.name.clone()
            };
            let tokens = crate::tui::format_tokens(p.tokens);
            let path_short = shorten_path(&p.path, 30);

            Row::new(vec![
                Cell::from(name).style(Style::default().fg(Color::White)),
                Cell::from(p.session_count.to_string()),
                Cell::from(p.tools.clone()).style(Style::default().fg(Color::DarkGray)),
                Cell::from(tokens).style(Style::default().fg(Color::Yellow)),
                Cell::from(path_short).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(23),
            Constraint::Length(10),
            Constraint::Length(16),
            Constraint::Length(10),
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

fn render_detail(f: &mut Frame, project: &ProjectDisplay, session_state: &mut TableState) {
    let area = f.area();

    let chunks = Layout::vertical([
        Constraint::Length(9),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .split(area);

    render_project_summary(f, project, chunks[0]);

    let header = Row::new(vec![
        Cell::from("Source"),
        Cell::from("Updated"),
        Cell::from("Tokens"),
        Cell::from("Title"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = project
        .sessions
        .iter()
        .map(|s| {
            let tokens = crate::tui::format_tokens(s.tokens);
            let name = if s.name.len() > 40 {
                format!("{}…", &s.name[..39])
            } else {
                s.name.clone()
            };
            let src = crate::tui::source_label(&s.source);

            Row::new(vec![
                Cell::from(src.to_string()),
                Cell::from(s.updated.clone()).style(Style::default().fg(Color::DarkGray)),
                Cell::from(tokens).style(Style::default().fg(Color::Yellow)),
                Cell::from(name),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        format!(" Sessions ({}) ", project.sessions.len()),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(table, chunks[1], session_state);

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
        Span::styled(" view session  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "q/esc",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" back", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

fn render_session(
    f: &mut Frame,
    sd: &SessionDisplay,
    lines: &[Line<'static>],
    scroll: u16,
    stats: Option<&StatsRow>,
) {
    let area = f.area();
    let header_height: u16 = if stats.is_some() { 7 } else { 4 };

    let chunks = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let src = crate::tui::source_label(&sd.source);
    let title_text = if sd.name.is_empty() {
        "(untitled)"
    } else {
        &sd.name
    };

    let mut header_lines = vec![
        Line::from(vec![
            Span::styled(
                " Session ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if sd.id.len() > 8 { &sd.id[..8] } else { &sd.id },
                Style::default().fg(Color::White),
            ),
            Span::styled(format!("  [{src}]"), Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![Span::styled(
            format!(" {title_text}"),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!(" Updated: {}", sd.updated),
            Style::default().fg(Color::DarkGray),
        )]),
    ];

    if let Some(st) = stats {
        let model_str = st.model.as_deref().unwrap_or("unknown");
        let inp = st.input_tokens.unwrap_or(0);
        let out = st.output_tokens.unwrap_or(0);
        let cost = st.total_cost_usd.unwrap_or(0.0);
        let dur = st.duration_secs.map(format_duration).unwrap_or_default();
        let tools = st.tool_call_count;
        let files = st.files_touched.len();

        header_lines.push(Line::from(""));
        let mut spans = vec![
            Span::styled(" Model: ", Style::default().fg(Color::DarkGray)),
            Span::styled(model_str.to_string(), Style::default().fg(Color::Magenta)),
            Span::styled("  Tokens: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}in / {}out", format_tokens(inp), format_tokens(out)),
                Style::default().fg(Color::Yellow),
            ),
        ];
        if cost > 0.001 {
            spans.push(Span::styled(
                "  Cost: ",
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                format!("${:.4}", cost),
                Style::default().fg(Color::Green),
            ));
        }
        header_lines.push(Line::from(spans));

        let mut detail = vec![];
        if !dur.is_empty() {
            detail.push(Span::styled(
                " Duration: ",
                Style::default().fg(Color::DarkGray),
            ));
            detail.push(Span::styled(dur, Style::default().fg(Color::White)));
        }
        if tools > 0 {
            detail.push(Span::styled(
                "  Tools: ",
                Style::default().fg(Color::DarkGray),
            ));
            detail.push(Span::styled(
                tools.to_string(),
                Style::default().fg(Color::White),
            ));
        }
        if files > 0 {
            detail.push(Span::styled(
                "  Files: ",
                Style::default().fg(Color::DarkGray),
            ));
            detail.push(Span::styled(
                files.to_string(),
                Style::default().fg(Color::White),
            ));
        }
        if !detail.is_empty() {
            header_lines.push(Line::from(detail));
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
        format!(" {:.0}%", (scroll as f64 / max * 100.0).min(100.0))
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
            "q/esc",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" back", Style::default().fg(Color::DarkGray)),
        Span::styled(scroll_pct, Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

fn render_project_summary(f: &mut Frame, p: &ProjectDisplay, area: Rect) {
    let total_tokens = crate::tui::format_tokens(p.tokens);
    let input = crate::tui::format_tokens(p.input_tokens);
    let output = crate::tui::format_tokens(p.output_tokens);
    let lines = vec![
        kv_line("Path", &shorten_path(&p.path, 60)),
        kv_line("Tools", &p.tools),
        kv_line("Sessions", &p.session_count.to_string()),
        kv_line(
            "Tokens",
            &format!("{input} in / {output} out ({total_tokens} total)"),
        ),
    ];

    let title = Line::from(vec![Span::styled(
        format!(" {} ", p.name),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]);

    let block = Block::bordered().title(title);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn format_tokens(n: i64) -> String {
    crate::tui::format_tokens(n)
}

fn format_duration(secs: i64) -> String {
    crate::tui::format_duration(secs)
}

fn shorten_path(path: &str, max: usize) -> String {
    if path.len() <= max {
        return path.to_string();
    }
    let home = dirs::home_dir()
        .and_then(|h| h.to_str().map(String::from))
        .unwrap_or_default();
    let shortened = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    };
    if shortened.len() <= max {
        shortened
    } else {
        format!("…{}", &shortened[shortened.len() - max + 1..])
    }
}
