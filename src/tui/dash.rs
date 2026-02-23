use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::config::Config;
use crate::cursor;
use crate::server;

struct ToolCount {
    label: &'static str,
    count: usize,
}

pub fn run(cfg: &Config) -> Result<(), String> {
    let project_root = cursor::get_project_root();

    let tools: Vec<ToolCount> = TOOL_LIST
        .iter()
        .filter(|(key, _)| tool_enabled(cfg, key))
        .map(|(key, label)| ToolCount {
            label,
            count: tool_session_count(key, &project_root),
        })
        .collect();

    let server_status = if cfg.server.api_key.is_empty() {
        "not configured — run oobo setup".to_string()
    } else {
        match server::check_connection(cfg) {
            Ok(msg) => msg,
            Err(e) => format!("error ({e})"),
        }
    };

    let mut terminal = crate::tui::init().map_err(|e| e.to_string())?;

    loop {
        let pr = &project_root;
        let ss = &server_status;
        let tc = &tools;
        terminal
            .draw(|f| render(f, cfg, pr, tc, ss))
            .map_err(|e| e.to_string())?;

        if let Some(KeyCode::Char('q') | KeyCode::Esc) =
            crate::tui::key_code(Duration::from_millis(100)).map_err(|e| e.to_string())?
        {
            break;
        }
    }

    crate::tui::restore();
    Ok(())
}

const TOOL_LIST: &[(&str, &str)] = &[
    ("cursor", "Cursor"),
    ("claude", "Claude"),
    ("windsurf", "Windsurf"),
    ("trae", "Trae"),
    ("aider", "Aider"),
    ("continue", "Continue"),
    ("copilot", "Copilot"),
    ("zed", "Zed"),
    ("codex", "Codex"),
];

fn tool_enabled(cfg: &Config, key: &str) -> bool {
    match key {
        "cursor" => cfg.cursor.enabled,
        "claude" => cfg.claude.enabled,
        "windsurf" => cfg.windsurf.enabled,
        "trae" => cfg.trae.enabled,
        "aider" => cfg.aider.enabled,
        "continue" => cfg.continue_dev.enabled,
        "copilot" => cfg.copilot.enabled,
        "zed" => cfg.zed.enabled,
        "codex" => cfg.codex.enabled,
        _ => false,
    }
}

fn tool_session_count(key: &str, root: &str) -> usize {
    let result = match key {
        "cursor" => cursor::sessions_for_project(root).map(|s| s.len()),
        "claude" => crate::claude::sessions_for_project(root).map(|s| s.len()),
        "windsurf" => crate::windsurf::sessions_for_project(root).map(|s| s.len()),
        "trae" => crate::trae::sessions_for_project(root).map(|s| s.len()),
        "aider" => crate::aider::sessions_for_project(root).map(|s| s.len()),
        "continue" => crate::continue_dev::sessions_for_project(root).map(|s| s.len()),
        "copilot" => crate::copilot::sessions_for_project(root).map(|s| s.len()),
        "zed" => crate::zed::sessions_for_project(root).map(|s| s.len()),
        "codex" => crate::codex::sessions_for_project(root).map(|s| s.len()),
        _ => Ok(0),
    };
    result.unwrap_or(0)
}

fn render(
    f: &mut Frame,
    cfg: &Config,
    project_root: &str,
    tools: &[ToolCount],
    server_status: &str,
) {
    let area = f.area();

    let tool_rows = tools.len().max(1) as u16;
    let config_height = 10u16;
    let tools_height = tool_rows + 2;
    let server_height = 3u16;

    let chunks = Layout::vertical([
        Constraint::Length(config_height),
        Constraint::Length(tools_height),
        Constraint::Length(server_height),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    let config_lines = vec![
        kv_line("Config file", &Config::config_path().display().to_string()),
        kv_line("Server URL", &cfg.server.url),
        kv_line(
            "API key",
            if cfg.server.api_key.is_empty() {
                "(not set)"
            } else {
                "••••••••"
            },
        ),
        kv_line("Git path", cfg.git_path()),
        kv_line(
            "Alias",
            if cfg.git.alias_enabled {
                "enabled"
            } else {
                "disabled"
            },
        ),
        kv_line(
            "Telemetry",
            if cfg.telemetry.enabled {
                "enabled"
            } else {
                "disabled"
            },
        ),
        kv_line("AI tools", &format!("{} enabled", tools.len())),
    ];
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let config_block = Block::bordered().title(Line::from(vec![
        Span::styled(
            " oobo ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(version, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
    ]));
    f.render_widget(Paragraph::new(config_lines).block(config_block), chunks[0]);

    let tool_lines: Vec<Line<'static>> = tools
        .iter()
        .map(|t| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<14} ", t.label),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{} sessions", t.count)),
            ])
        })
        .collect();

    let tools_block = Block::bordered().title(Line::from(vec![
        Span::styled(
            " AI Tools ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("• {project_root} "),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    f.render_widget(Paragraph::new(tool_lines).block(tools_block), chunks[1]);

    let server_lines = vec![kv_line("Status", server_status)];
    let server_block = Block::bordered().title(Span::styled(
        " Server ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    f.render_widget(Paragraph::new(server_lines).block(server_block), chunks[2]);

    let footer = Line::from(vec![
        Span::styled(
            " q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[4]);
}

fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<14} "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(value.to_string()),
    ])
}
