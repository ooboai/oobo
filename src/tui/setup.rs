use std::collections::HashSet;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::config::Config;

pub struct ScanInfo {
    pub detected: Vec<(String, usize)>,
    pub projects: usize,
    pub sessions: usize,
    pub project_choices: Vec<ProjectChoice>,
}

impl ScanInfo {
    fn detected_keys(&self) -> HashSet<&str> {
        self.detected.iter().map(|(k, _)| k.as_str()).collect()
    }
}

#[derive(Clone, Debug)]
pub struct ProjectChoice {
    pub id: String,
    pub name: String,
    pub path: String,
    pub tools: Vec<String>,
    pub sessions: usize,
    pub enabled: bool,
}

pub struct SetupOutcome {
    pub config: Config,
    pub projects: Vec<ProjectChoice>,
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Welcome,
    Projects,
    Save,
}

impl Step {
    fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Projects => "Projects",
            Self::Save => "Save",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Welcome => 0,
            Self::Projects => 1,
            Self::Save => 2,
        }
    }
}

const STEP_COUNT: usize = 3;

struct ToolToggle {
    config_key: &'static str,
    #[allow(dead_code)]
    display_name: &'static str,
    #[allow(dead_code)]
    category: &'static str,
    enabled: bool,
    #[allow(dead_code)]
    detected: bool,
}

struct Wizard {
    step: Step,
    tools: Vec<ToolToggle>,
    projects: Vec<ProjectChoice>,
    base_cfg: Config,
    focus: usize,
    scan: ScanInfo,
}

enum Action {
    Continue,
    Save(Box<SetupOutcome>),
    Cancel,
}

impl Wizard {
    fn new(cfg: &Config, scan: ScanInfo) -> Self {
        let detected_keys = scan.detected_keys();
        let reg = crate::tools::registry();

        let mut tools: Vec<ToolToggle> = reg
            .all()
            .map(|t| {
                let key = t.config_key();
                let detected = detected_keys.contains(key);
                ToolToggle {
                    config_key: key,
                    display_name: t.display_name(),
                    category: t.category(),
                    enabled: t.enabled(cfg) || detected,
                    detected,
                }
            })
            .collect();

        tools.sort_by_key(|t| if t.category == "ide" { 0 } else { 1 });

        let mut projects = scan.project_choices.clone();
        for p in &mut projects {
            if p.sessions > 0 && !p.enabled {
                p.enabled = true;
            }
        }

        Wizard {
            step: Step::Welcome,
            tools,
            projects,
            base_cfg: cfg.clone(),
            focus: 0,
            scan,
        }
    }

    fn to_config(&self) -> Config {
        let mut cfg = self.base_cfg.clone();
        for tool in &self.tools {
            cfg.set_tool_enabled(tool.config_key, tool.enabled);
        }
        cfg
    }

    fn to_outcome(&self) -> SetupOutcome {
        SetupOutcome {
            config: self.to_config(),
            projects: self.projects.clone(),
        }
    }

    fn enabled_tool_count(&self) -> usize {
        self.tools.iter().filter(|t| t.enabled).count()
    }

    fn detected_tool_count(&self) -> usize {
        self.tools.iter().filter(|t| t.detected).count()
    }

    fn enabled_project_count(&self) -> usize {
        self.projects.iter().filter(|p| p.enabled).count()
    }

}

/// Build a default outcome without showing the TUI wizard (for non-interactive environments).
pub fn build_default_outcome(cfg: &Config, scan: ScanInfo) -> SetupOutcome {
    Wizard::new(cfg, scan).to_outcome()
}

pub fn run_setup_wizard(cfg: &Config, scan: ScanInfo) -> Result<Option<SetupOutcome>, String> {
    let mut wiz = Wizard::new(cfg, scan);
    let mut terminal = crate::tui::init().map_err(|e| e.to_string())?;

    let result = loop {
        terminal
            .draw(|f| render(f, &wiz))
            .map_err(|e| e.to_string())?;

        if let Some(key) = crate::tui::next_key(crate::tui::KEY_POLL).map_err(|e| e.to_string())? {
            match handle_key(&mut wiz, key.code) {
                Action::Continue => continue,
                Action::Save(cfg) => break Ok(Some(*cfg)),
                Action::Cancel => break Ok(None),
            }
        }
    };

    crate::tui::restore();
    result
}

fn handle_key(wiz: &mut Wizard, code: KeyCode) -> Action {
    if code == KeyCode::Char('q') {
        return Action::Cancel;
    }

    match wiz.step {
        Step::Welcome => match code {
            KeyCode::Enter | KeyCode::Tab => {
                wiz.step = Step::Projects;
                wiz.focus = 0;
            }
            KeyCode::Esc => return Action::Cancel,
            _ => {}
        },
        Step::Projects => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                let max = wiz.projects.len().saturating_sub(1);
                if wiz.focus < max {
                    wiz.focus += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                wiz.focus = wiz.focus.saturating_sub(1);
            }
            KeyCode::Char(' ') if wiz.focus < wiz.projects.len() => {
                wiz.projects[wiz.focus].enabled = !wiz.projects[wiz.focus].enabled;
            }
            KeyCode::Char('a') => {
                let all_enabled = wiz.projects.iter().all(|p| p.enabled);
                for project in &mut wiz.projects {
                    project.enabled = !all_enabled;
                }
            }
            KeyCode::Enter | KeyCode::Tab => {
                wiz.step = Step::Save;
                wiz.focus = 0;
            }
            KeyCode::Esc | KeyCode::BackTab => {
                wiz.step = Step::Welcome;
                wiz.focus = 0;
            }
            _ => {}
        },
        Step::Save => match code {
            KeyCode::Char('s') | KeyCode::Enter => {
                return Action::Save(Box::new(wiz.to_outcome()));
            }
            KeyCode::Esc | KeyCode::BackTab => {
                wiz.step = Step::Projects;
                wiz.focus = 0;
            }
            _ => {}
        },
    }

    Action::Continue
}

fn render(f: &mut Frame, wiz: &Wizard) {
    let area = f.area();
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let step_label = wiz.step.label();
    let title = format!(
        " anchor setup · {step_label} ({}/{STEP_COUNT}) ",
        wiz.step.index() + 1
    );
    let block = Block::bordered().title(Line::from(vec![Span::styled(
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));

    let inner = block.inner(chunks[0]);

    match wiz.step {
        Step::Welcome => render_welcome(f, inner, wiz),
        Step::Projects => render_projects(f, inner, wiz),
        Step::Save => render_save(f, inner, wiz),
    }

    f.render_widget(block, chunks[0]);
    f.render_widget(Paragraph::new(hint_line(wiz)), chunks[1]);
}

fn render_welcome(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let green = Style::default().fg(Color::Green);

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled("  Welcome to anchor.", bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ", dim),
            Span::raw("anchor enriches your git commits with AI session tracking,"),
        ]),
        Line::from(vec![
            Span::styled("  ", dim),
            Span::raw("token analytics, and code attribution across all your tools."),
        ]),
        Line::from(""),
    ];

    if wiz.scan.projects > 0 || wiz.scan.sessions > 0 {
        lines.push(Line::from(vec![
            Span::raw("  Scan: "),
            Span::styled(
                format!(
                    "{} project(s), {} session(s) found",
                    wiz.scan.projects, wiz.scan.sessions
                ),
                green,
            ),
        ]));
        lines.push(Line::from(Span::styled(
            "  Choose which projects to enable in the next step.",
            dim,
        )));
        lines.push(Line::from(""));
    }

    if !wiz.scan.detected.is_empty() {
        lines.push(Line::from(Span::styled("  Detected tools:", dim)));
        for (key, count) in &wiz.scan.detected {
            let display = wiz
                .tools
                .iter()
                .find(|t| t.config_key == key)
                .map(|t| t.display_name)
                .unwrap_or(key.as_str());
            lines.push(Line::from(vec![
                Span::styled("    ✓ ", green),
                Span::raw(format!("{display:<16}")),
                Span::styled(format!("{count} session(s)"), dim),
            ]));
        }
        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(Span::styled(
            "  No AI tool sessions detected yet.",
            dim,
        )));
        lines.push(Line::from(Span::styled(
            "  You can enable tools in the next step.",
            dim,
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled("  Press ", dim),
        Span::styled(
            "enter",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to continue", dim),
    ]));

    f.render_widget(Paragraph::new(lines), area);
}

fn render_projects(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let green = Style::default().fg(Color::Green);

    let enabled = wiz.enabled_project_count();
    let total = wiz.projects.len();
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Select projects to track.",
            bold,
        )),
        Line::from(Span::styled(
            "  Projects with AI sessions are pre-enabled. Toggle with space, a for all.",
            dim,
        )),
        Line::from(""),
    ];

    if wiz.projects.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No projects found. anchor will discover projects as you use AI tools.",
            dim,
        )));
        lines.push(Line::from(Span::styled(
            "  Enable any repo later with `anchor enable` from inside it.",
            dim,
        )));
    } else {
        let max_items = area.height.saturating_sub(7).max(1) as usize;
        let start = if wiz.focus >= max_items {
            wiz.focus + 1 - max_items
        } else {
            0
        };
        let end = (start + max_items).min(wiz.projects.len());

        for (i, project) in wiz.projects[start..end].iter().enumerate() {
            lines.push(project_line(start + i, project, wiz.focus));
        }

        if end < wiz.projects.len() {
            lines.push(Line::from(Span::styled(
                format!("  ↓ {} more", wiz.projects.len() - end),
                dim,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ", dim),
        Span::styled(format!("{enabled}"), green),
        Span::styled(format!(" of {total} projects enabled"), dim),
    ]));

    f.render_widget(Paragraph::new(lines), area);
}

fn project_line(idx: usize, p: &ProjectChoice, focus: usize) -> Line<'static> {
    let focused = idx == focus;
    let marker = if focused { "▸ " } else { "  " };
    let check = if p.enabled { "✓" } else { " " };
    let check_style = if p.enabled {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let label_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if p.enabled {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let path_display = display_project_path(&p.path);

    let tools_str = if p.tools.is_empty() {
        String::new()
    } else {
        let abbrevs: Vec<&str> = p.tools.iter().take(3).map(|t| tool_abbrev(t)).collect();
        format!("  {}", abbrevs.join(" "))
    };

    Line::from(vec![
        Span::raw(format!("  {marker}")),
        Span::styled(format!("[{check}]"), check_style),
        Span::styled(format!(" {:<18}", truncate_str(&p.name, 18)), label_style),
        Span::styled(
            format!("  {:>4} sessions", p.sessions),
            Style::default().fg(if p.sessions > 0 { Color::Gray } else { Color::DarkGray }),
        ),
        Span::styled(tools_str, Style::default().fg(Color::DarkGray)),
        Span::styled(format!("  {path_display}"), Style::default().fg(Color::DarkGray)),
    ])
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn display_project_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

fn tool_abbrev(key: &str) -> &str {
    match key {
        "cursor" => "cur",
        "claude" => "cc",
        "codex" => "cdx",
        "copilot" => "cop",
        "gemini" => "gem",
        "aider" => "aid",
        "continue" => "cnt",
        "opencode" => "opc",
        "zed" => "zed",
        "windsurf" => "wnd",
        "amp" => "amp",
        "junie" => "jun",
        "kiro" => "kir",
        "trae" => "tra",
        "droid" => "drd",
        other => other,
    }
}


fn render_save(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let green = Style::default().fg(Color::Green);

    let enabled = wiz.enabled_tool_count();
    let detected = wiz.detected_tool_count();

    let tools_summary: Vec<String> = wiz
        .tools
        .iter()
        .filter(|t| t.enabled)
        .take(3)
        .map(|t| t.display_name.to_string())
        .collect();
    let extra = if enabled > 3 {
        format!(" + {} more", enabled - 3)
    } else {
        String::new()
    };
    let tools_str = format!("{}{extra}", tools_summary.join(", "));

    let p_enabled = wiz.enabled_project_count();
    let p_total = wiz.projects.len();

    let lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled("  Review your configuration:", bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Projects:      ", dim),
            Span::styled(format!("{p_enabled}"), green),
            Span::styled(format!(" of {p_total} enabled"), dim),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tools:         ", dim),
            Span::styled(format!("{enabled}"), green),
            Span::styled(format!(" enabled ({detected} detected)"), dim),
        ]),
        Line::from(vec![
            Span::styled("                 ", dim),
            Span::raw(tools_str),
        ]),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", dim),
            Span::styled(
                "enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" or ", dim),
            Span::styled(
                "s",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to save configuration.", dim),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), area);
}

fn hint_line(wiz: &Wizard) -> Line<'static> {
    let key_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let mut spans: Vec<Span<'static>> = Vec::new();

    match wiz.step {
        Step::Welcome => {
            spans.extend([
                Span::styled(" enter", key_style),
                Span::styled(" continue  ", dim),
                Span::styled("q", key_style),
                Span::styled(" cancel", dim),
            ]);
        }
        Step::Projects => {
            spans.extend([
                Span::styled(" ↑↓", key_style),
                Span::styled(" navigate  ", dim),
                Span::styled("space", key_style),
                Span::styled(" toggle  ", dim),
                Span::styled("a", key_style),
                Span::styled(" all  ", dim),
                Span::styled("enter", key_style),
                Span::styled(" next  ", dim),
                Span::styled("esc", key_style),
                Span::styled(" back  ", dim),
                Span::styled("q", key_style),
                Span::styled(" cancel", dim),
            ]);
        }
        Step::Save => {
            spans.extend([
                Span::styled(" enter", key_style),
                Span::styled("/", dim),
                Span::styled("s", key_style),
                Span::styled(" save  ", dim),
                Span::styled("esc", key_style),
                Span::styled(" back  ", dim),
                Span::styled("q", key_style),
                Span::styled(" cancel", dim),
            ]);
        }
    }

    Line::from(spans)
}
