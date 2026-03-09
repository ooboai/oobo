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
}

impl ScanInfo {
    fn detected_keys(&self) -> HashSet<&str> {
        self.detected.iter().map(|(k, _)| k.as_str()).collect()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Welcome,
    Tools,
    Sync,
    Cloud,
    Alias,
    Save,
}

impl Step {
    fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Tools => "Tools",
            Self::Sync => "Session sync",
            Self::Cloud => "Cloud",
            Self::Alias => "Git alias",
            Self::Save => "Save",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Welcome => 0,
            Self::Tools => 1,
            Self::Sync => 2,
            Self::Cloud => 3,
            Self::Alias => 4,
            Self::Save => 5,
        }
    }
}

const STEP_COUNT: usize = 6;

struct ToolToggle {
    config_key: &'static str,
    display_name: &'static str,
    category: &'static str,
    enabled: bool,
    detected: bool,
    session_count: usize,
}

const SYNC_OPTIONS: [(&str, &str, &str); 2] = [
    (
        "off",
        "Metadata only",
        "Anchor metadata syncs with repos. Session transcripts stay local.",
    ),
    (
        "on",
        "Full transparency",
        "Anchor metadata + redacted session transcripts sync with repos.",
    ),
];

struct Wizard {
    step: Step,
    tools: Vec<ToolToggle>,
    git_alias: bool,
    git_path: String,
    sync_choice: usize,
    cloud_enabled: bool,
    base_cfg: Config,
    focus: usize,
    scan: ScanInfo,
}

enum Action {
    Continue,
    Save(Box<Config>),
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
                let session_count = scan
                    .detected
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, c)| *c)
                    .unwrap_or(0);
                ToolToggle {
                    config_key: key,
                    display_name: t.display_name(),
                    category: t.category(),
                    enabled: t.enabled(cfg) || detected,
                    detected,
                    session_count,
                }
            })
            .collect();

        tools.sort_by_key(|t| if t.category == "ide" { 0 } else { 1 });

        let sync_choice = match cfg.transparency.mode.as_str() {
            "on" | "full" | "full_transparency" => 1,
            _ => 0,
        };

        let git_path = crate::config::find_real_git().unwrap_or_else(|| "git".into());
        let cloud_enabled = !cfg.server.api_key.is_empty();

        Wizard {
            step: Step::Welcome,
            tools,
            git_alias: cfg.git.alias_enabled,
            git_path,
            sync_choice,
            cloud_enabled,
            base_cfg: cfg.clone(),
            focus: 0,
            scan,
        }
    }

    fn to_config(&self) -> Config {
        let mut cfg = self.base_cfg.clone();
        cfg.git.real_git_path = self.git_path.clone();
        cfg.git.alias_enabled = self.git_alias;
        for tool in &self.tools {
            cfg.set_tool_enabled(tool.config_key, tool.enabled);
        }
        cfg.transparency.mode = SYNC_OPTIONS[self.sync_choice].0.to_string();
        cfg
    }

    fn enabled_count(&self) -> usize {
        self.tools.iter().filter(|t| t.enabled).count()
    }

    fn detected_count(&self) -> usize {
        self.tools.iter().filter(|t| t.detected).count()
    }

    fn sync_label(&self) -> &'static str {
        SYNC_OPTIONS[self.sync_choice].1
    }

    fn ide_tools(&self) -> impl Iterator<Item = (usize, &ToolToggle)> {
        self.tools
            .iter()
            .enumerate()
            .filter(|(_, t)| t.category == "ide")
    }

    fn cli_tools(&self) -> impl Iterator<Item = (usize, &ToolToggle)> {
        self.tools
            .iter()
            .enumerate()
            .filter(|(_, t)| t.category == "cli")
    }
}

pub fn run_setup_wizard(cfg: &Config, scan: ScanInfo) -> Result<Option<Config>, String> {
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
                wiz.step = Step::Tools;
                wiz.focus = 0;
            }
            KeyCode::Esc => return Action::Cancel,
            _ => {}
        },
        Step::Tools => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                let max = wiz.tools.len().saturating_sub(1);
                if wiz.focus < max {
                    wiz.focus += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                wiz.focus = wiz.focus.saturating_sub(1);
            }
            KeyCode::Char(' ') => {
                if wiz.focus < wiz.tools.len() {
                    wiz.tools[wiz.focus].enabled = !wiz.tools[wiz.focus].enabled;
                }
            }
            KeyCode::Enter | KeyCode::Tab => {
                wiz.step = Step::Sync;
                wiz.focus = wiz.sync_choice;
            }
            KeyCode::Esc | KeyCode::BackTab => {
                wiz.step = Step::Welcome;
                wiz.focus = 0;
            }
            _ => {}
        },
        Step::Sync => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                if wiz.focus < SYNC_OPTIONS.len() - 1 {
                    wiz.focus += 1;
                    wiz.sync_choice = wiz.focus;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                wiz.focus = wiz.focus.saturating_sub(1);
                wiz.sync_choice = wiz.focus;
            }
            KeyCode::Char(' ') => {
                wiz.sync_choice = wiz.focus;
            }
            KeyCode::Enter | KeyCode::Tab => {
                wiz.sync_choice = wiz.focus;
                wiz.step = Step::Cloud;
                wiz.focus = 0;
            }
            KeyCode::Esc | KeyCode::BackTab => {
                wiz.step = Step::Tools;
                wiz.focus = 0;
            }
            _ => {}
        },
        Step::Cloud => match code {
            KeyCode::Char(' ') | KeyCode::Enter => {
                wiz.cloud_enabled = !wiz.cloud_enabled;
            }
            KeyCode::Tab => {
                wiz.step = Step::Alias;
                wiz.focus = 0;
            }
            KeyCode::Esc | KeyCode::BackTab => {
                wiz.step = Step::Sync;
                wiz.focus = wiz.sync_choice;
            }
            _ => {}
        },
        Step::Alias => match code {
            KeyCode::Char(' ') | KeyCode::Enter => {
                wiz.git_alias = !wiz.git_alias;
            }
            KeyCode::Tab => {
                wiz.step = Step::Save;
                wiz.focus = 0;
            }
            KeyCode::Esc | KeyCode::BackTab => {
                wiz.step = Step::Cloud;
                wiz.focus = 0;
            }
            _ => {}
        },
        Step::Save => match code {
            KeyCode::Char('s') | KeyCode::Enter => {
                return Action::Save(Box::new(wiz.to_config()));
            }
            KeyCode::Esc | KeyCode::BackTab => {
                wiz.step = Step::Alias;
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
        " oobo setup · {step_label} ({}/{STEP_COUNT}) ",
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
        Step::Tools => render_tools(f, inner, wiz),
        Step::Sync => render_sync(f, inner, wiz),
        Step::Cloud => render_cloud(f, inner, wiz),
        Step::Alias => render_alias(f, inner, wiz),
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
        Line::from(Span::styled("  Welcome to oobo.", bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ", dim),
            Span::raw("oobo enriches your git commits with AI session tracking,"),
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
                bold,
            ),
        ]));
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

fn render_tools(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);
    let green = Style::default().fg(Color::Green);
    let header = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Select which AI tools to track. Detected tools are pre-enabled.",
            dim,
        )),
        Line::from(""),
        Line::from(Span::styled("  IDE Editors", header)),
    ];

    for (i, t) in wiz.ide_tools() {
        lines.push(tool_line(i, t, wiz.focus));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  CLI Agents", header)));

    for (i, t) in wiz.cli_tools() {
        lines.push(tool_line(i, t, wiz.focus));
    }

    lines.push(Line::from(""));

    let enabled = wiz.enabled_count();
    let detected = wiz.detected_count();
    lines.push(Line::from(vec![
        Span::styled("  ", dim),
        Span::styled(format!("{enabled}"), green),
        Span::styled(" enabled", dim),
        if detected > 0 {
            Span::styled(format!(" ({detected} detected)"), dim)
        } else {
            Span::raw("")
        },
    ]));

    f.render_widget(Paragraph::new(lines), area);
}

fn tool_line(idx: usize, t: &ToolToggle, focus: usize) -> Line<'static> {
    let focused = idx == focus;
    let marker = if focused { "▸ " } else { "  " };
    let check = if t.enabled { "✓" } else { " " };
    let check_style = if t.enabled {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let label_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let mut spans = vec![
        Span::raw(format!("  {marker}")),
        Span::styled(format!("{:<16}", t.display_name), label_style),
        Span::styled(format!("[{check}]"), check_style),
    ];

    if t.detected {
        spans.push(Span::styled(
            format!("  {} session(s)", t.session_count),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}

fn render_sync(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Should oobo sync AI session metadata with your git repos?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  When syncing is on, teammates who clone the repo can see which",
            dim,
        )),
        Line::from(Span::styled(
            "  AI sessions contributed to each commit. You can change this later.",
            dim,
        )),
        Line::from(""),
    ];

    for (i, (_, label, description)) in SYNC_OPTIONS.iter().enumerate() {
        let selected = i == wiz.sync_choice;
        let focused = i == wiz.focus;
        let marker = if focused { "▸" } else { " " };
        let radio = if selected { "●" } else { "○" };

        let radio_style = if selected {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let label_style = if focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };

        lines.push(Line::from(vec![
            Span::raw(format!("  {marker} ")),
            Span::styled(format!("{radio} "), radio_style),
            Span::styled(label.to_string(), label_style),
        ]));

        lines.push(Line::from(Span::styled(
            format!("       {description}"),
            dim,
        )));
        lines.push(Line::from(""));
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_cloud(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let check = if wiz.cloud_enabled { "✓" } else { " " };
    let check_style = if wiz.cloud_enabled {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Want to connect to the oobo dashboard?",
            bold,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  The dashboard gives you a web UI for your AI usage across projects",
            dim,
        )),
        Line::from(Span::styled(
            "  and teams. This is optional. Everything works without it.",
            dim,
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  ▸ "),
            Span::styled("Enable cloud sync ", bold),
            Span::styled(format!("[{check}]"), check_style),
        ]),
        Line::from(""),
    ];

    if wiz.cloud_enabled {
        lines.push(Line::from(Span::styled(
            "  After setup, authenticate with:",
            dim,
        )));
        lines.push(Line::from(Span::styled(
            "    oobo auth login",
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "  No account yet? Sign up at:",
        dim,
    )));
    lines.push(Line::from(Span::styled(
        "    https://oobo.ai/auth/signup",
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Self-hosting? Set a custom endpoint after setup:",
        dim,
    )));
    lines.push(Line::from(Span::styled(
        "    oobo auth set-remote https://your-server.com",
        Style::default().fg(Color::Cyan),
    )));

    f.render_widget(Paragraph::new(lines), area);
}

fn render_alias(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let check = if wiz.git_alias { "✓" } else { " " };
    let check_style = if wiz.git_alias {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Want to alias git to oobo?",
            bold,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  When enabled, typing `git commit` automatically enriches your",
            dim,
        )),
        Line::from(Span::styled(
            "  commits with AI context. All git commands work exactly as before.",
            dim,
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  ▸ "),
            Span::styled("Enable git alias ", bold),
            Span::styled(format!("[{check}]"), check_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Without the alias, you can still use `oobo commit` directly.",
            dim,
        )),
        Line::from(Span::styled(
            "  Undo anytime with `oobo alias uninstall`.",
            dim,
        )),
    ];

    f.render_widget(Paragraph::new(lines), area);
}

fn render_save(f: &mut Frame, area: ratatui::layout::Rect, wiz: &Wizard) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let green = Style::default().fg(Color::Green);

    let enabled = wiz.enabled_count();
    let detected = wiz.detected_count();

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

    let alias_str = if wiz.git_alias {
        "enabled (git = oobo)"
    } else {
        "disabled"
    };

    let lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(Span::styled("  Review your configuration:", bold)),
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
        Line::from(vec![
            Span::styled("  Session sync:  ", dim),
            Span::raw(wiz.sync_label().to_string()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Cloud:         ", dim),
            Span::raw(
                if wiz.cloud_enabled {
                    "enabled (run `oobo auth login` to authenticate)"
                } else {
                    "skipped"
                }
                .to_string(),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Git alias:     ", dim),
            Span::raw(alias_str.to_string()),
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
        Step::Tools => {
            spans.extend([
                Span::styled(" ↑↓", key_style),
                Span::styled(" navigate  ", dim),
                Span::styled("space", key_style),
                Span::styled(" toggle  ", dim),
                Span::styled("enter", key_style),
                Span::styled(" next  ", dim),
                Span::styled("esc", key_style),
                Span::styled(" back  ", dim),
                Span::styled("q", key_style),
                Span::styled(" cancel", dim),
            ]);
        }
        Step::Sync => {
            spans.extend([
                Span::styled(" ↑↓", key_style),
                Span::styled(" select  ", dim),
                Span::styled("enter", key_style),
                Span::styled(" next  ", dim),
                Span::styled("esc", key_style),
                Span::styled(" back  ", dim),
                Span::styled("q", key_style),
                Span::styled(" cancel", dim),
            ]);
        }
        Step::Cloud => {
            spans.extend([
                Span::styled(" space", key_style),
                Span::styled(" toggle  ", dim),
                Span::styled("tab", key_style),
                Span::styled(" next  ", dim),
                Span::styled("esc", key_style),
                Span::styled(" back  ", dim),
                Span::styled("q", key_style),
                Span::styled(" cancel", dim),
            ]);
        }
        Step::Alias => {
            spans.extend([
                Span::styled(" space", key_style),
                Span::styled(" toggle  ", dim),
                Span::styled("tab", key_style),
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
