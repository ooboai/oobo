use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use super::kv_line;
use crate::analytics::git_activity::{GitActivityRow, ProductivitySummary};
use crate::db::ai_commits::{AiCodeHeadline, AiCommitSummary, AiWeeklyTrend};
use crate::db::stats::{AggregateStats, ModelStats};
use crate::tools::cursor::composer_data::DailyCodeStats;

pub struct StatsView {
    pub scope_label: String,
    pub global: AggregateStats,
    pub per_tool: Vec<(String, AggregateStats)>,
    pub top_projects: Vec<(String, AggregateStats)>,
    pub daily: Vec<(String, AggregateStats)>,
    pub data_sources: Vec<(String, String)>,
    pub cursor_code_stats: Vec<DailyCodeStats>,
    pub ai_commits: Option<AiCommitSummary>,
    pub ai_branches: Vec<(String, AiCommitSummary)>,
    pub ai_headline: Option<AiCodeHeadline>,
    pub ai_weekly_trend: Vec<AiWeeklyTrend>,
    pub per_model: Vec<ModelStats>,
    pub productivity: Option<ProductivitySummary>,
    pub git_activity: Vec<GitActivityRow>,
}

pub fn run(view: StatsView) -> Result<(), String> {
    let mut terminal = crate::tui::init().map_err(|e| e.to_string())?;
    let mut scroll: u16 = 0;

    loop {
        let v = &view;
        let sc = scroll;
        terminal
            .draw(|f| render(f, v, sc))
            .map_err(|e| e.to_string())?;

        if let Some(code) =
            crate::tui::key_code(Duration::from_millis(100)).map_err(|e| e.to_string())?
        {
            match code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => scroll = scroll.saturating_add(10),
                KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                _ => {}
            }
        }
    }

    crate::tui::restore();
    Ok(())
}

fn render(f: &mut Frame, view: &StatsView, _scroll: u16) {
    let area = f.area();

    let has_tools = !view.per_tool.is_empty();
    let has_models = !view.per_model.is_empty();
    let has_projects = !view.top_projects.is_empty();
    let has_daily = !view.daily.is_empty();
    let has_code_stats = !view.cursor_code_stats.is_empty();
    let has_ai_commits = view
        .ai_commits
        .as_ref()
        .is_some_and(|a| a.total_commits > 0);
    let has_ai_headline = view
        .ai_headline
        .as_ref()
        .is_some_and(|h| h.total_commits > 0);
    let has_ai_branches = !view.ai_branches.is_empty();
    let has_ai_trend = !view.ai_weekly_trend.is_empty();
    let has_productivity = view
        .productivity
        .as_ref()
        .is_some_and(|p| p.total_commits > 0);
    let has_git_activity = !view.git_activity.is_empty();
    let tool_rows = view.per_tool.len() as u16;
    let model_rows = view.per_model.len().min(10) as u16;
    let project_rows = view.top_projects.len().min(10) as u16;
    let daily_rows = view.daily.len().min(14) as u16;
    let code_stats_rows = view.cursor_code_stats.len().min(10) as u16;
    let branch_rows = view.ai_branches.len().min(8) as u16;
    let trend_rows = view.ai_weekly_trend.len().min(8) as u16;
    let git_rows = view.git_activity.len().min(14) as u16;

    let summary_height = if view.data_sources.is_empty() { 9 } else { 10 };
    let mut constraints = vec![Constraint::Length(summary_height)];

    if has_ai_headline || has_ai_commits {
        constraints.push(Constraint::Length(10));
    }
    if has_ai_trend {
        constraints.push(Constraint::Length(trend_rows + 4));
    }
    if has_productivity {
        constraints.push(Constraint::Length(9));
    }
    if has_tools {
        constraints.push(Constraint::Length(tool_rows + 4));
    }
    if has_models {
        constraints.push(Constraint::Length(model_rows + 4));
    }
    if has_daily {
        constraints.push(Constraint::Length(daily_rows + 4));
    }
    if has_git_activity {
        constraints.push(Constraint::Length(git_rows + 4));
    }
    if has_projects {
        constraints.push(Constraint::Length(project_rows + 4));
    }
    if has_ai_branches {
        constraints.push(Constraint::Length(branch_rows + 4));
    }
    if has_code_stats {
        constraints.push(Constraint::Length(code_stats_rows + 4));
    }
    constraints.push(Constraint::Min(0));
    constraints.push(Constraint::Length(1));

    let chunks = Layout::vertical(constraints).split(area);

    render_summary(f, view, chunks[0]);

    let mut idx = 1;

    if has_ai_headline || has_ai_commits {
        render_ai_attribution(
            f,
            view.ai_commits.as_ref(),
            view.ai_headline.as_ref(),
            chunks[idx],
        );
        idx += 1;
    }
    if has_ai_trend {
        render_ai_trend_table(f, &view.ai_weekly_trend, chunks[idx]);
        idx += 1;
    }
    if has_productivity {
        render_productivity(f, view.productivity.as_ref().unwrap(), chunks[idx]);
        idx += 1;
    }
    if has_tools {
        render_tool_table(f, &view.per_tool, chunks[idx]);
        idx += 1;
    }
    if has_models {
        render_model_table(f, &view.per_model, chunks[idx]);
        idx += 1;
    }
    if has_daily {
        render_daily_table(f, &view.daily, chunks[idx]);
        idx += 1;
    }
    if has_git_activity {
        render_git_activity_table(f, &view.git_activity, chunks[idx]);
        idx += 1;
    }
    if has_projects {
        render_project_table(f, &view.top_projects, chunks[idx]);
        idx += 1;
    }
    if has_ai_branches {
        render_ai_branch_table(f, &view.ai_branches, chunks[idx]);
        idx += 1;
    }
    if has_code_stats {
        render_code_stats_table(f, &view.cursor_code_stats, chunks[idx]);
        idx += 1;
    }

    let _ = idx;

    let footer_idx = chunks.len() - 1;
    let footer = Line::from(vec![
        Span::styled(
            " q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" quit  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "j/k",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "oobo index",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        Span::styled(" to refresh", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[footer_idx]);
}

fn render_summary(f: &mut Frame, view: &StatsView, area: Rect) {
    let g = &view.global;
    let total_tokens = g.total_input_tokens + g.total_output_tokens;

    let mut lines = vec![
        kv_line("Sessions", &g.session_count.to_string()),
        kv_line(
            "Input tokens",
            &crate::tui::format_tokens(g.total_input_tokens),
        ),
        kv_line(
            "Output tokens",
            &crate::tui::format_tokens(g.total_output_tokens),
        ),
        kv_line("Total tokens", &crate::tui::format_tokens(total_tokens)),
        kv_line(
            "Total time",
            &if g.total_duration_secs > 0 {
                crate::tui::format_duration(g.total_duration_secs)
            } else {
                "—".to_string()
            },
        ),
    ];

    if !view.data_sources.is_empty() {
        let sources: Vec<String> = view
            .data_sources
            .iter()
            .map(|(tool, src)| format!("{tool}: {src}"))
            .collect();
        lines.push(kv_line_dim("Data", &sources.join(" · ")));
    }

    let title = Line::from(vec![
        Span::styled(
            " oobo stats ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("• {} ", view.scope_label),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let block = Block::bordered().title(title);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_tool_table(f: &mut Frame, tools: &[(String, AggregateStats)], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Tool"),
        Cell::from("Sessions"),
        Cell::from("Input"),
        Cell::from("Output"),
        Cell::from("Time"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = tools
        .iter()
        .map(|(source, s)| {
            let label = crate::tui::source_label(source);
            let time = if s.total_duration_secs > 0 {
                crate::tui::format_duration(s.total_duration_secs)
            } else {
                "—".to_string()
            };
            Row::new(vec![
                Cell::from(label.to_string()).style(Style::default().fg(Color::White)),
                Cell::from(s.session_count.to_string()),
                Cell::from(crate::tui::format_tokens(s.total_input_tokens)),
                Cell::from(crate::tui::format_tokens(s.total_output_tokens)),
                Cell::from(time),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " Per Tool ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

fn render_daily_table(f: &mut Frame, daily: &[(String, AggregateStats)], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Date"),
        Cell::from("Sessions"),
        Cell::from("Input"),
        Cell::from("Output"),
        Cell::from("Total"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = daily
        .iter()
        .take(14)
        .map(|(date, s)| {
            let total = s.total_input_tokens + s.total_output_tokens;

            let bar_len = if total > 0 {
                let max_tokens: i64 = daily
                    .iter()
                    .map(|(_, d)| d.total_input_tokens + d.total_output_tokens)
                    .max()
                    .unwrap_or(1);
                ((total as f64 / max_tokens as f64) * 8.0).ceil() as usize
            } else {
                0
            };
            let bar = "█".repeat(bar_len);

            Row::new(vec![
                Cell::from(date.clone()).style(Style::default().fg(Color::White)),
                Cell::from(s.session_count.to_string()),
                Cell::from(crate::tui::format_tokens(s.total_input_tokens)),
                Cell::from(crate::tui::format_tokens(s.total_output_tokens)),
                Cell::from(format!("{} {bar}", crate::tui::format_tokens(total)))
                    .style(Style::default().fg(Color::Cyan)),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " Recent Activity ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

fn render_project_table(f: &mut Frame, projects: &[(String, AggregateStats)], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Project"),
        Cell::from("Sessions"),
        Cell::from("Input"),
        Cell::from("Output"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = projects
        .iter()
        .take(10)
        .map(|(name, s)| {
            let display = if name.len() > 25 {
                format!("{}…", &name[..24])
            } else {
                name.clone()
            };
            Row::new(vec![
                Cell::from(display).style(Style::default().fg(Color::White)),
                Cell::from(s.session_count.to_string()),
                Cell::from(crate::tui::format_tokens(s.total_input_tokens)),
                Cell::from(crate::tui::format_tokens(s.total_output_tokens)),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " Top Projects ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(26),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Min(12),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

fn kv_line_dim(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<16} "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::DarkGray)),
    ])
}

fn render_ai_attribution(
    f: &mut Frame,
    ai: Option<&AiCommitSummary>,
    headline: Option<&AiCodeHeadline>,
    area: Rect,
) {
    let (ai_pct, total_commits, total_added, ai_added, human_added, tab_added) =
        if let Some(h) = headline {
            let pct = h.ai_percentage;
            (
                pct,
                h.total_commits,
                h.total_lines,
                h.ai_lines,
                h.human_lines,
                0i64,
            )
        } else if let Some(a) = ai {
            let pct = if a.total_lines_added > 0 {
                100.0 * a.ai_lines_added as f64 / a.total_lines_added as f64
            } else {
                0.0
            };
            (
                pct,
                a.total_commits,
                a.total_lines_added,
                a.ai_lines_added,
                a.human_lines_added,
                a.tab_lines_added,
            )
        } else {
            return;
        };

    let human_pct = 100.0 - ai_pct;

    let bar_width = 20;
    let ai_bars = ((ai_pct / 100.0) * bar_width as f64).round() as usize;
    let human_bars = bar_width - ai_bars;
    let bar = format!("{}{}", "█".repeat(ai_bars), "░".repeat(human_bars));

    let mut lines = vec![
        kv_line("Commits scored", &total_commits.to_string()),
        kv_line("Lines added", &fmt_num(total_added)),
        kv_line(
            "AI-assisted",
            &format!("{} ({:.1}%)", fmt_num(ai_added), ai_pct),
        ),
        kv_line(
            "Human-written",
            &format!("{} ({:.1}%)", fmt_num(human_added), human_pct),
        ),
    ];

    if tab_added > 0 {
        lines.push(kv_line("Tab completions", &fmt_num(tab_added)));
    }

    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:<16} ", "AI vs Human"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(bar, Style::default().fg(Color::Magenta)),
        Span::styled(
            format!(" {:.0}% AI", ai_pct),
            Style::default().fg(Color::White),
        ),
    ]));

    let title_text = format!(" AI Code Attribution — {:.0}% AI-assisted ", ai_pct);
    let block = Block::bordered().title(Span::styled(
        title_text,
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_ai_branch_table(f: &mut Frame, branches: &[(String, AiCommitSummary)], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Branch"),
        Cell::from("Commits"),
        Cell::from("Lines+"),
        Cell::from("AI+"),
        Cell::from("AI%"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = branches
        .iter()
        .take(8)
        .map(|(name, s)| {
            let display = if name.len() > 28 {
                format!("{}…", &name[..27])
            } else {
                name.clone()
            };
            let ai_pct = if s.total_lines_added > 0 {
                format!(
                    "{:.0}%",
                    100.0 * s.ai_lines_added as f64 / s.total_lines_added as f64
                )
            } else {
                "—".to_string()
            };
            let pct_color = if s.ai_lines_added as f64 / s.total_lines_added.max(1) as f64 > 0.8 {
                Color::Magenta
            } else if s.ai_lines_added > 0 {
                Color::Cyan
            } else {
                Color::DarkGray
            };
            Row::new(vec![
                Cell::from(display).style(Style::default().fg(Color::White)),
                Cell::from(s.total_commits.to_string()),
                Cell::from(fmt_num(s.total_lines_added)),
                Cell::from(fmt_num(s.ai_lines_added)),
                Cell::from(ai_pct).style(Style::default().fg(pct_color)),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " AI Attribution by Branch ",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(29),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

fn render_model_table(f: &mut Frame, models: &[ModelStats], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Model"),
        Cell::from("Sessions"),
        Cell::from("Input"),
        Cell::from("Output"),
        Cell::from("% Total"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = models
        .iter()
        .take(10)
        .map(|m| {
            let display = shorten_model(&m.model, 22);
            let pct = format!("{:.1}%", m.pct_of_total_output);
            Row::new(vec![
                Cell::from(display).style(Style::default().fg(Color::White)),
                Cell::from(m.session_count.to_string()),
                Cell::from(crate::tui::format_tokens(m.input_tokens)),
                Cell::from(crate::tui::format_tokens(m.output_tokens)),
                Cell::from(pct).style(Style::default().fg(Color::Cyan)),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " Per Model ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(23),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

fn render_ai_trend_table(f: &mut Frame, trends: &[AiWeeklyTrend], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Week"),
        Cell::from("Commits"),
        Cell::from("Lines+"),
        Cell::from("AI Lines"),
        Cell::from("Human"),
        Cell::from("AI%"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = trends
        .iter()
        .take(8)
        .map(|t| {
            let pct = t.ai_percentage();
            let pct_str = format!("{:.0}%", pct);
            let pct_color = if pct >= 70.0 {
                Color::Magenta
            } else if pct >= 40.0 {
                Color::Cyan
            } else {
                Color::DarkGray
            };
            Row::new(vec![
                Cell::from(t.week.clone()).style(Style::default().fg(Color::White)),
                Cell::from(t.commits.to_string()),
                Cell::from(fmt_num(t.lines_added)),
                Cell::from(fmt_num(t.ai_lines)),
                Cell::from(fmt_num(t.human_lines)),
                Cell::from(pct_str).style(Style::default().fg(pct_color)),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " AI Code Trend (Weekly) ",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

fn render_productivity(f: &mut Frame, prod: &ProductivitySummary, area: Rect) {
    let cpd = prod.commits_per_day();
    let lpd = prod.lines_per_day();

    let lines = vec![
        kv_line("Active days", &prod.active_days.to_string()),
        kv_line("Total commits", &fmt_num(prod.total_commits)),
        kv_line("Lines added", &fmt_num(prod.total_lines_added)),
        kv_line("Lines deleted", &fmt_num(prod.total_lines_deleted)),
        kv_line("Commits/day", &format!("{:.1}", cpd)),
        kv_line("Lines/day", &format!("{:.0}", lpd)),
    ];

    let block = Block::bordered().title(Span::styled(
        " Git Productivity ",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_git_activity_table(f: &mut Frame, activity: &[GitActivityRow], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Date"),
        Cell::from("Commits"),
        Cell::from("Lines+"),
        Cell::from("Lines-"),
        Cell::from("Files"),
        Cell::from("AI"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = activity
        .iter()
        .take(14)
        .map(|a| {
            let ai_str = if a.ai_assisted_commits > 0 {
                a.ai_assisted_commits.to_string()
            } else {
                "—".to_string()
            };
            let bar_len = {
                let max_lines: i64 = activity.iter().map(|d| d.lines_added).max().unwrap_or(1);
                if a.lines_added > 0 && max_lines > 0 {
                    ((a.lines_added as f64 / max_lines as f64) * 6.0).ceil() as usize
                } else {
                    0
                }
            };
            let bar = "█".repeat(bar_len);

            Row::new(vec![
                Cell::from(a.date.clone()).style(Style::default().fg(Color::White)),
                Cell::from(a.commits.to_string()),
                Cell::from(format!("{} {bar}", fmt_num(a.lines_added)))
                    .style(Style::default().fg(Color::Green)),
                Cell::from(fmt_num(a.lines_deleted)),
                Cell::from(a.files_changed.to_string()),
                Cell::from(ai_str),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " Git Activity ",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

fn shorten_model(model: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    if model.len() <= max_len {
        return model.to_string();
    }
    let end = model
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i < max_len)
        .last()
        .unwrap_or(0);
    format!("{}…", &model[..end])
}

fn fmt_num(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn render_code_stats_table(f: &mut Frame, stats: &[DailyCodeStats], area: Rect) {
    let header = Row::new(vec![
        Cell::from("Date"),
        Cell::from("Suggested"),
        Cell::from("Accepted"),
        Cell::from("Rate"),
        Cell::from("Tab"),
        Cell::from("Agent"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = stats
        .iter()
        .take(10)
        .map(|s| {
            let rate = s.acceptance_rate();
            let rate_str = if rate > 0.0 {
                format!("{:.0}%", rate * 100.0)
            } else {
                "—".to_string()
            };
            let rate_color = if rate >= 0.5 {
                Color::Green
            } else if rate > 0.0 {
                Color::Yellow
            } else {
                Color::DarkGray
            };

            Row::new(vec![
                Cell::from(s.date.clone()).style(Style::default().fg(Color::White)),
                Cell::from(s.total_suggested().to_string()),
                Cell::from(s.total_accepted().to_string()),
                Cell::from(rate_str).style(Style::default().fg(rate_color)),
                Cell::from(format!(
                    "{}/{}",
                    s.tab_accepted_lines, s.tab_suggested_lines
                )),
                Cell::from(format!(
                    "{}/{}",
                    s.composer_accepted_lines, s.composer_suggested_lines
                )),
            ])
        })
        .collect();

    let block = Block::bordered().title(Span::styled(
        " Cursor AI Code Stats ",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(14),
            Constraint::Length(14),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_model_short() {
        assert_eq!(shorten_model("gpt-4o", 10), "gpt-4o");
    }

    #[test]
    fn test_shorten_model_exact() {
        assert_eq!(shorten_model("gpt-4o-min", 10), "gpt-4o-min");
    }

    #[test]
    fn test_shorten_model_long() {
        let result = shorten_model("claude-3.5-sonnet-20260101", 10);
        assert!(result.ends_with('…'));
        assert!(result.len() <= 10 + '…'.len_utf8());
    }

    #[test]
    fn test_shorten_model_zero_max() {
        assert_eq!(shorten_model("anything", 0), "");
    }

    #[test]
    fn test_shorten_model_multibyte() {
        let result = shorten_model("café-model", 4);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_fmt_num_below_thousand() {
        assert_eq!(fmt_num(42), "42");
    }

    #[test]
    fn test_fmt_num_thousands() {
        assert_eq!(fmt_num(1500), "1.5K");
    }

    #[test]
    fn test_fmt_num_millions() {
        assert_eq!(fmt_num(2_500_000), "2.5M");
    }
}
