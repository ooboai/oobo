use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::config::{find_real_git, Config};

const FIELD_COUNT: usize = 15;

struct TextInput {
    value: String,
    cursor: usize,
}

impl TextInput {
    #[allow(dead_code)]
    fn new(initial: &str) -> Self {
        let cursor = initial.chars().count();
        Self {
            value: initial.to_string(),
            cursor,
        }
    }

    fn handle_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                let byte_pos = self.byte_offset(self.cursor);
                self.value.insert(byte_pos, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let start = self.byte_offset(self.cursor - 1);
                    let end = self.byte_offset(self.cursor);
                    self.value.drain(start..end);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let max = self.value.chars().count();
                if self.cursor < max {
                    let start = self.byte_offset(self.cursor);
                    let end = self.byte_offset(self.cursor + 1);
                    self.value.drain(start..end);
                }
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let max = self.value.chars().count();
                if self.cursor < max {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.chars().count(),
            _ => {}
        }
    }

    fn byte_offset(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    fn to_spans(&self, editing: bool) -> Vec<Span<'static>> {
        if !editing {
            if self.value.is_empty() {
                return vec![Span::styled(
                    "(empty)",
                    Style::default().fg(Color::DarkGray),
                )];
            }
            return vec![Span::raw(self.value.clone())];
        }

        let chars: Vec<char> = self.value.chars().collect();
        let before: String = chars[..self.cursor].iter().collect();
        let cursor_ch = if self.cursor < chars.len() {
            chars[self.cursor].to_string()
        } else {
            " ".to_string()
        };
        let after: String = if self.cursor < chars.len() {
            chars[self.cursor + 1..].iter().collect()
        } else {
            String::new()
        };

        vec![
            Span::raw(before),
            Span::styled(cursor_ch, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ]
    }
}

struct SetupApp {
    fields: [FieldState; FIELD_COUNT],
    focused: usize,
    editing: bool,
    git_path: String,
}

#[derive(Clone)]
enum FieldState {
    Text {
        label: &'static str,
        value: String,
        cursor: usize,
    },
    Toggle {
        label: &'static str,
        value: bool,
    },
    Display {
        label: &'static str,
        value: String,
    },
}

impl SetupApp {
    fn new(cfg: &Config) -> Self {
        let git_path = find_real_git().unwrap_or_else(|| "git".into());
        Self {
            fields: [
                FieldState::Text {
                    label: "Dashboard URL",
                    value: cfg.server.url.clone(),
                    cursor: cfg.server.url.chars().count(),
                },
                FieldState::Text {
                    label: "API key",
                    value: cfg.server.api_key.clone(),
                    cursor: cfg.server.api_key.chars().count(),
                },
                FieldState::Display {
                    label: "Git path",
                    value: git_path.clone(),
                },
                FieldState::Toggle {
                    label: "Git alias",
                    value: cfg.git.alias_enabled,
                },
                FieldState::Toggle {
                    label: "Cursor",
                    value: cfg.cursor.enabled,
                },
                FieldState::Toggle {
                    label: "Claude Code",
                    value: cfg.claude.enabled,
                },
                FieldState::Toggle {
                    label: "Windsurf",
                    value: cfg.windsurf.enabled,
                },
                FieldState::Toggle {
                    label: "Aider",
                    value: cfg.aider.enabled,
                },
                FieldState::Toggle {
                    label: "Continue.dev",
                    value: cfg.continue_dev.enabled,
                },
                FieldState::Toggle {
                    label: "Copilot Chat",
                    value: cfg.copilot.enabled,
                },
                FieldState::Toggle {
                    label: "Zed AI",
                    value: cfg.zed.enabled,
                },
                FieldState::Toggle {
                    label: "Trae",
                    value: cfg.trae.enabled,
                },
                FieldState::Toggle {
                    label: "Codex",
                    value: cfg.codex.enabled,
                },
                FieldState::Toggle {
                    label: "OpenCode",
                    value: cfg.opencode.enabled,
                },
                FieldState::Toggle {
                    label: "Telemetry",
                    value: cfg.telemetry.enabled,
                },
            ],
            focused: 0,
            editing: false,
            git_path,
        }
    }

    fn is_editable(&self, idx: usize) -> bool {
        !matches!(self.fields[idx], FieldState::Display { .. })
    }

    fn next_editable(&mut self) {
        let start = self.focused;
        loop {
            self.focused = (self.focused + 1) % FIELD_COUNT;
            if self.is_editable(self.focused) || self.focused == start {
                break;
            }
        }
    }

    fn prev_editable(&mut self) {
        let start = self.focused;
        loop {
            self.focused = if self.focused == 0 {
                FIELD_COUNT - 1
            } else {
                self.focused - 1
            };
            if self.is_editable(self.focused) || self.focused == start {
                break;
            }
        }
    }

    fn toggle_val(&self, idx: usize) -> bool {
        match &self.fields[idx] {
            FieldState::Toggle { value, .. } => *value,
            _ => true,
        }
    }

    fn to_config(&self) -> Config {
        let url = match &self.fields[0] {
            FieldState::Text { value, .. } => value.clone(),
            _ => String::new(),
        };
        let api_key = match &self.fields[1] {
            FieldState::Text { value, .. } => value.clone(),
            _ => String::new(),
        };

        Config {
            server: crate::config::ServerConfig { url, api_key },
            git: crate::config::GitConfig {
                real_git_path: self.git_path.clone(),
                alias_enabled: self.toggle_val(3),
            },
            cursor: crate::config::ToolConfig {
                enabled: self.toggle_val(4),
            },
            claude: crate::config::ToolConfig {
                enabled: self.toggle_val(5),
            },
            windsurf: crate::config::ToolConfig {
                enabled: self.toggle_val(6),
            },
            aider: crate::config::ToolConfig {
                enabled: self.toggle_val(7),
            },
            continue_dev: crate::config::ToolConfig {
                enabled: self.toggle_val(8),
            },
            copilot: crate::config::ToolConfig {
                enabled: self.toggle_val(9),
            },
            zed: crate::config::ToolConfig {
                enabled: self.toggle_val(10),
            },
            trae: crate::config::ToolConfig {
                enabled: self.toggle_val(11),
            },
            codex: crate::config::ToolConfig {
                enabled: self.toggle_val(12),
            },
            opencode: crate::config::ToolConfig {
                enabled: self.toggle_val(13),
            },
            telemetry: crate::config::TelemetryConfig {
                enabled: self.toggle_val(14),
                send_diffs: false,
            },
        }
    }
}

/// Returns Ok(true) if saved, Ok(false) if cancelled.
pub fn run_setup_tui() -> Result<bool, String> {
    let cfg = Config::load_or_default();
    let mut app = SetupApp::new(&cfg);

    let mut terminal = crate::tui::init().map_err(|e| e.to_string())?;

    let result = loop {
        terminal
            .draw(|f| render(f, &app))
            .map_err(|e| e.to_string())?;

        if let Some(key) =
            crate::tui::next_key(Duration::from_millis(50)).map_err(|e| e.to_string())?
        {
            if app.editing {
                match key.code {
                    KeyCode::Enter | KeyCode::Esc => {
                        app.editing = false;
                    }
                    other => {
                        if let FieldState::Text { value, cursor, .. } = &mut app.fields[app.focused]
                        {
                            let mut input = TextInput {
                                value: value.clone(),
                                cursor: *cursor,
                            };
                            input.handle_key(other);
                            *value = input.value;
                            *cursor = input.cursor;
                        }
                    }
                }
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(false),
                    KeyCode::Char('s')
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            || key.modifiers.is_empty() =>
                    {
                        let new_cfg = app.to_config();
                        new_cfg.save()?;

                        if new_cfg.git.alias_enabled {
                            let _ = crate::alias::install_alias();
                        }

                        break Ok(true);
                    }
                    KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                        app.next_editable();
                    }
                    KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                        app.prev_editable();
                    }
                    KeyCode::Enter => match &mut app.fields[app.focused] {
                        FieldState::Text { .. } => {
                            app.editing = true;
                        }
                        FieldState::Toggle { value, .. } => {
                            *value = !*value;
                        }
                        _ => {}
                    },
                    KeyCode::Char(' ') => {
                        if let FieldState::Toggle { value, .. } = &mut app.fields[app.focused] {
                            *value = !*value;
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    crate::tui::restore();
    result
}

fn render(f: &mut Frame, app: &SetupApp) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(2 + FIELD_COUNT as u16 + 1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, field) in app.fields.iter().enumerate() {
        let focused = i == app.focused;
        let marker = if focused { "▸ " } else { "  " };
        let label_style = if focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        match field {
            FieldState::Text {
                label,
                value,
                cursor,
            } => {
                let editing = focused && app.editing;
                let input = TextInput {
                    value: value.clone(),
                    cursor: *cursor,
                };
                let value_spans = input.to_spans(editing);
                let mut spans = vec![
                    Span::raw(marker.to_string()),
                    Span::styled(format!("{label:<14} "), label_style),
                ];
                spans.extend(value_spans);
                if editing {
                    spans.push(Span::styled(
                        "  (editing)",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.push(Line::from(spans));
            }
            FieldState::Toggle { label, value } => {
                let check = if *value { "✓" } else { " " };
                let check_style = if *value {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                lines.push(Line::from(vec![
                    Span::raw(marker.to_string()),
                    Span::styled(format!("{label:<14} "), label_style),
                    Span::styled(format!("[{check}]"), check_style),
                    Span::raw(if *value { " enabled" } else { " disabled" }.to_string()),
                ]));
            }
            FieldState::Display { label, value } => {
                lines.push(Line::from(vec![
                    Span::raw("  ".to_string()),
                    Span::styled(
                        format!("{label:<14} "),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(value.clone()),
                    Span::styled("  (auto)", Style::default().fg(Color::DarkGray)),
                ]));
            }
        }
    }

    let block = Block::bordered().title(Line::from(vec![Span::styled(
        " oobo setup ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));
    f.render_widget(Paragraph::new(lines).block(block), chunks[0]);

    let hint = if app.editing {
        Line::from(vec![
            Span::styled(
                " type",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to edit  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" confirm  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
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
            Span::styled(" edit  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "space",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" toggle  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "s",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" save  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ])
    };
    f.render_widget(Paragraph::new(hint), chunks[2]);
}
