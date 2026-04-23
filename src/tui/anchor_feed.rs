//! Unified anchor-feed TUI.
//!
//! The single TUI surface for bare `oobo` (pretty mode) inside an enabled
//! repo. Replaces the v0 per-command TUIs (dash, sessions, projects, stats).
//!
//! Controls:
//! - `↑` / `↓` or `j` / `k` — move selection
//! - `g` / `G` — jump to top / bottom
//! - `q` or `Esc` — quit

use std::io;
use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::config::Config;
use crate::db::Db;

pub struct AnchorRow {
    pub sha: String,
    pub timestamp: i64,
    pub subject: String,
    pub tool: Option<String>,
    pub tokens: Option<i64>,
    pub session_count: usize,
}

pub struct FeedHeader {
    pub project_name: String,
    pub anchor_count: usize,
    pub total_tokens: i64,
    pub ai_pct: i64,
    pub enabled: bool,
}

pub fn run(cfg: &Config) -> Result<i32, String> {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        return Err("not a git repository".to_string());
    };
    let project_id = crate::paths::slug_from_path(&root);
    let project_name = std::path::Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let db = Db::open()?;
    let settings = db.get_project_settings(&project_id).unwrap_or_default();
    let stats = db.anchor_stats_for_project(&project_id).unwrap_or_default();

    let header = FeedHeader {
        project_name,
        anchor_count: stats.anchors as usize,
        total_tokens: stats.tokens,
        ai_pct: stats.ai_pct,
        enabled: !settings.ignored,
    };

    let rows = load_rows(&db, &project_id, 200)?;

    if !header.enabled {
        // Disabled project → skip TUI, one-line hint.
        println!("oobo disabled for this project. run: oobo enable");
        return Ok(0);
    }

    let mut terminal = super::init().map_err(|e| format!("tui init: {e}"))?;
    let result = event_loop(&mut terminal, &header, &rows);
    super::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    header: &FeedHeader,
    rows: &[AnchorRow],
) -> Result<i32, String> {
    let mut state = ListState::default();
    if !rows.is_empty() {
        state.select(Some(0));
    }

    loop {
        terminal
            .draw(|frame| draw(frame, header, rows, &mut state))
            .map_err(|e| format!("tui draw: {e}"))?;

        if let Some(key) = super::key_code(Duration::from_millis(200))
            .map_err(|e: io::Error| format!("key read: {e}"))?
        {
            match key {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(0),
                KeyCode::Down | KeyCode::Char('j') => next(&mut state, rows.len()),
                KeyCode::Up | KeyCode::Char('k') => prev(&mut state, rows.len()),
                KeyCode::Char('g') => {
                    if !rows.is_empty() {
                        state.select(Some(0));
                    }
                }
                KeyCode::Char('G') => {
                    if !rows.is_empty() {
                        state.select(Some(rows.len() - 1));
                    }
                }
                _ => {}
            }
        }
    }
}

fn draw(
    frame: &mut ratatui::Frame<'_>,
    header: &FeedHeader,
    rows: &[AnchorRow],
    state: &mut ListState,
) {
    let size = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);

    let header_line = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" oobo · {}", header.project_name),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "    {} anchors · {} tok · {}% AI",
                header.anchor_count,
                super::format_tokens(header.total_tokens),
                header.ai_pct
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .alignment(Alignment::Left);
    frame.render_widget(header_line, layout[0]);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| {
            let when = relative_time(r.timestamp);
            let tool = r.tool.clone().unwrap_or_else(|| "-".to_string());
            let tokens = r
                .tokens
                .map(super::format_tokens)
                .unwrap_or_else(|| "-".to_string());
            ListItem::new(Line::from(vec![
                Span::styled(" ● ", Style::default().fg(Color::Green)),
                Span::styled(format!("{when:<5} "), Style::default().fg(Color::DarkGray)),
                Span::raw(truncate(&r.subject, 38)),
                Span::styled(
                    format!("   {tool} · {tokens} · {} sess", r.session_count),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(list, layout[1], state);

    let footer = Paragraph::new(Line::from(vec![Span::styled(
        " ↑↓ nav · q quit ",
        Style::default().fg(Color::DarkGray),
    )]))
    .alignment(Alignment::Left);
    frame.render_widget(footer, layout[2]);
}

fn next(state: &mut ListState, len: usize) {
    if len == 0 {
        return;
    }
    let i = state.selected().map(|i| (i + 1) % len).unwrap_or(0);
    state.select(Some(i));
}

fn prev(state: &mut ListState, len: usize) {
    if len == 0 {
        return;
    }
    let i = state
        .selected()
        .map(|i| if i == 0 { len - 1 } else { i - 1 })
        .unwrap_or(0);
    state.select(Some(i));
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        format!("{s:<max$}")
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn relative_time(ts: i64) -> String {
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
    } else if d < 30 * 86400 {
        format!("{}d", d / 86400)
    } else {
        format!("{}mo", d / (30 * 86400))
    }
}

fn load_rows(db: &Db, project_id: &str, limit: usize) -> Result<Vec<AnchorRow>, String> {
    use rusqlite::params;
    let mut rows: Vec<AnchorRow> = Vec::new();
    let mut stmt = db
        .conn
        .prepare(
            "SELECT a.commit_hash, a.committed_at, a.message, a.intent
             FROM anchors a
             JOIN ai_commits c ON c.commit_hash = a.commit_hash
             WHERE c.project_id = ?1
             ORDER BY a.committed_at DESC
             LIMIT ?2",
        )
        .map_err(|e| format!("prepare feed: {e}"))?;
    let mapped = stmt
        .query_map(params![project_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| format!("feed query: {e}"))?;
    for r in mapped.flatten() {
        let (hash, ts, msg, intent) = r;
        let subject = intent
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| msg.unwrap_or_else(|| "(no subject)".to_string()));
        let (tool, tokens, session_count) = summarize_sessions(db, &hash);
        rows.push(AnchorRow {
            sha: hash,
            timestamp: ts.unwrap_or(0),
            subject,
            tool,
            tokens,
            session_count,
        });
    }
    Ok(rows)
}

fn summarize_sessions(db: &Db, commit_hash: &str) -> (Option<String>, Option<i64>, usize) {
    use rusqlite::params;
    let mut count = 0usize;
    let mut tokens: i64 = 0;
    let mut tool: Option<String> = None;
    let sql = "SELECT agent, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens
               FROM anchor_sessions WHERE commit_hash = ?1";
    if let Ok(mut stmt) = db.conn.prepare(sql) {
        if let Ok(rows) = stmt.query_map(params![commit_hash], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            ))
        }) {
            for r in rows.flatten() {
                count += 1;
                tokens += r.1 + r.2 + r.3 + r.4;
                if tool.is_none() {
                    tool = Some(r.0);
                }
            }
        }
    }
    (tool, if tokens == 0 { None } else { Some(tokens) }, count)
}
