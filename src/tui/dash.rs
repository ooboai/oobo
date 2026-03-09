use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use super::kv_line;
use crate::config::Config;
use crate::remote;
use crate::tools::cursor;

struct ToolCount {
    label: String,
    count: usize,
}

pub fn run(cfg: &Config) -> Result<(), String> {
    let project_root = cursor::get_project_root();
    let reg = crate::tools::registry();

    let tools: Vec<ToolCount> = reg
        .enabled(cfg)
        .map(|t| ToolCount {
            label: t.display_name().to_string(),
            count: t
                .sessions_for_project(&project_root)
                .map(|s| s.len())
                .unwrap_or(0),
        })
        .collect();

    let server_status = if cfg.server.api_key.is_empty() {
        "not configured — run oobo setup".to_string()
    } else {
        match remote::check_connection(cfg) {
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
