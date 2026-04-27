//! Unified anchor-feed TUI — the flagship experience for bare `oobo`
//! inside an enabled repo.
//!
//! Architecture: a single [`App`] owns everything (anchors, filter, time
//! window, config) and a `Vec<View>` view-stack. The top of the stack is
//! the active view. `Esc`/`Backspace` pops; `q` quits from the root view.
//!
//! Views:
//! - [`View::Feed`]        two-pane list + detail
//! - [`View::Transcript`]  scrollable session messages with per-session cycling
//! - [`View::Search`]      in-TUI search (project/global toggle)
//! - [`View::Picker`]      modal list picker (session / file chooser)
//! - [`View::Help`]        keybindings overlay
//!
//! For some actions we suspend the TUI, shell out to an external tool
//! (git show, anchor blame), and restore on return.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::config::Config;

use super::format::*;
use super::transcript::load_transcript_lines;
use super::types::{
    AnchorRow, FeedState, MemoryKind, PickerAction, PickerKind, PickerState, SearchState,
    SessionLink, TimeWindow, TranscriptState, View,
};

// ── Public entry ──────────────────────────────────────────────────────

pub fn run(cfg: &Config) -> Result<i32, String> {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        return Err("not a git repository".to_string());
    };

    let mut terminal = super::init().map_err(|e| format!("tui init: {e}"))?;

    let project_name = std::path::Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();
    let enabled = crate::project_config::is_enabled(&root);
    let branch = current_branch(&root);
    let dirty = worktree_dirty(&root);
    let anchor_remote =
        crate::project_config::anchor_remote(&root).unwrap_or_else(|| "origin".to_string());

    let cfg_clone = cfg.clone();
    let root_clone = root.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = App::load(cfg_clone, root_clone);
        let _ = tx.send(result);
    });

    const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut frame_idx = 0usize;
    let skeleton = LoadingSkeleton {
        project_name: &project_name,
        enabled,
        branch: branch.as_deref(),
        dirty,
        anchor_remote: &anchor_remote,
    };
    let app = loop {
        let spinner = SPINNER_FRAMES[frame_idx];
        let _ = terminal.draw(|f| {
            draw_loading_skeleton(f, &skeleton, spinner);
        });

        match rx.try_recv() {
            Ok(result) => break result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                super::restore();
                return Err("loading thread panicked".to_string());
            }
        }

        if crossterm::event::poll(Duration::from_millis(80)).unwrap_or(false) {
            if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    super::restore();
                    return Ok(0);
                }
            }
        }
        frame_idx = (frame_idx + 1) % SPINNER_FRAMES.len();
    };

    let mut app = match app {
        Ok(app) => app,
        Err(e) => {
            super::restore();
            return Err(e);
        }
    };
    let result = event_loop(&mut terminal, &mut app);
    super::restore();
    result
}

struct LoadingSkeleton<'a> {
    project_name: &'a str,
    enabled: bool,
    branch: Option<&'a str>,
    dirty: bool,
    anchor_remote: &'a str,
}

fn draw_loading_skeleton(
    frame: &mut ratatui::Frame<'_>,
    sk: &LoadingSkeleton<'_>,
    spinner: &str,
) {
    let area = frame.area();

    let notice = !sk.enabled;
    let constraints = if notice {
        vec![
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let (header_area, notice_area, body_area, footer_area) = if notice {
        (layout[0], Some(layout[1]), layout[2], layout[3])
    } else {
        (layout[0], None, layout[1], layout[2])
    };

    // Header — identical to draw_header
    let branch_str = sk.branch.unwrap_or("detached");
    let dirty_label = if sk.dirty { "dirty" } else { "clean" };
    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                "  anchor",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" / ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                sk.project_name.to_string(),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("  memory", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            chip("branch", branch_str, Color::Cyan),
            Span::styled("  ", Style::default()),
            chip(
                "tree",
                dirty_label,
                if sk.dirty {
                    Color::Yellow
                } else {
                    Color::DarkGray
                },
            ),
            Span::styled("  ", Style::default()),
            chip("anchors", sk.anchor_remote, Color::Magenta),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{spinner} loading"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("   ", Style::default()),
            Span::styled("window all", Style::default().fg(Color::Gray)),
            Span::styled("   ", Style::default()),
            Span::styled(
                if sk.enabled {
                    "tracking on"
                } else {
                    "tracking off"
                },
                if sk.enabled {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
            Span::styled("   ", Style::default()),
            Span::styled(
                dirty_label,
                if sk.dirty {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(header_lines), header_area);

    if let Some(na) = notice_area {
        draw_tracking_notice(frame, na);
    }

    // Body — same 43/57 split as real feed, with skeleton placeholder lines
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
        .split(body_area);

    let list_block = Block::default().borders(Borders::NONE);
    let list_inner = list_block.inner(body[0]);
    frame.render_widget(list_block, body[0]);

    let shimmer_chars = ["░", "▒", "░", " "];
    let list_h = list_inner.height as usize;
    let list_w = list_inner.width.saturating_sub(4) as usize;
    let mut list_lines: Vec<Line<'_>> = Vec::new();
    for i in 0..list_h {
        if i < 8 {
            let bar_len = match i {
                0 => list_w * 75 / 100,
                1 => list_w * 60 / 100,
                2 => list_w * 80 / 100,
                3 => list_w * 55 / 100,
                4 => list_w * 70 / 100,
                5 => list_w * 45 / 100,
                6 => list_w * 65 / 100,
                7 => list_w * 50 / 100,
                _ => list_w * 60 / 100,
            };
            let ch = shimmer_chars[i % shimmer_chars.len()];
            let bar: String = ch.repeat(bar_len);
            list_lines.push(Line::from(Span::styled(
                format!("  {bar}"),
                Style::default().fg(Color::Rgb(40, 40, 50)),
            )));
        } else {
            list_lines.push(Line::from(""));
        }
    }
    frame.render_widget(Paragraph::new(list_lines), list_inner);

    let detail_block = Block::default().borders(Borders::NONE);
    let detail_inner = detail_block.inner(body[1]);
    frame.render_widget(detail_block, body[1]);

    let detail_h = detail_inner.height as usize;
    let mut detail_lines: Vec<Line<'_>> = Vec::new();
    for i in 0..detail_h {
        if i < 5 {
            let bar_len = match i {
                0 => 20,
                1 => 35,
                2 => 15,
                3 => 25,
                4 => 30,
                _ => 20,
            };
            let ch = shimmer_chars[(i + 2) % shimmer_chars.len()];
            let bar: String = ch.repeat(bar_len);
            detail_lines.push(Line::from(Span::styled(
                format!("  {bar}"),
                Style::default().fg(Color::Rgb(40, 40, 50)),
            )));
        } else {
            detail_lines.push(Line::from(""));
        }
    }
    frame.render_widget(Paragraph::new(detail_lines), detail_inner);

    // Footer — same keybindings as real feed
    let footer = Line::from(vec![
        Span::styled(" ↑↓", Style::default().fg(Color::White)),
        Span::styled(" move  ", Style::default().fg(Color::DarkGray)),
        Span::styled("enter", Style::default().fg(Color::White)),
        Span::styled(" memory  ", Style::default().fg(Color::DarkGray)),
        Span::styled("/", Style::default().fg(Color::White)),
        Span::styled(" filter  ", Style::default().fg(Color::DarkGray)),
        Span::styled("?", Style::default().fg(Color::White)),
        Span::styled(" help  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::White)),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(footer), footer_area);
}

#[cfg(test)]
pub fn run_projects(_cfg: &Config) -> Result<i32, String> {
    println!("anchor: not a git repository. cd into a project and run `anchor` to see your anchor feed.");
    Ok(0)
}

// ── App state ─────────────────────────────────────────────────────────

pub(super) struct App {
    cfg: Config,
    root: String,
    project_id: String,
    project_name: String,
    branch: Option<String>,
    anchor_remote: String,
    dirty: bool,
    pub(super) enabled: bool,
    anchors: Vec<AnchorRow>,
    filter: String,
    time_window: TimeWindow,
    stack: Vec<View>,
    flash: Option<String>,
}

// ── Loading ───────────────────────────────────────────────────────────

impl App {
    pub(super) fn load(cfg: Config, root: String) -> Result<Self, String> {
        let project_id = crate::project::id_for_root(&root);
        let project_name = std::path::Path::new(&root)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let enabled = crate::project_config::is_enabled(&root);
        let anchors = load_anchors(&cfg, &root, 500, TimeWindow::All)?;
        let branch = current_branch(&root);
        let anchor_remote =
            crate::project_config::anchor_remote(&root).unwrap_or_else(|| "origin".to_string());
        let dirty = worktree_dirty(&root);

        let mut feed = FeedState {
            list: ListState::default(),
            filter_input_open: false,
        };
        if !anchors.is_empty() {
            feed.list.select(Some(0));
        }

        Ok(Self {
            cfg,
            root,
            project_id,
            project_name,
            branch,
            anchor_remote,
            dirty,
            enabled,
            anchors,
            filter: String::new(),
            time_window: TimeWindow::All,
            stack: vec![View::Feed(feed)],
            flash: None,
        })
    }

    fn reload_anchors(&mut self) {
        if let Ok(rows) = load_anchors(&self.cfg, &self.root, 500, self.time_window) {
            self.anchors = rows;
        }
        self.enabled = crate::project_config::is_enabled(&self.root);
        self.branch = current_branch(&self.root);
        self.anchor_remote = crate::project_config::anchor_remote(&self.root)
            .unwrap_or_else(|| "origin".to_string());
        self.dirty = worktree_dirty(&self.root);
        if let Some(View::Feed(feed)) = self.stack.first_mut() {
            let visible = visible_anchor_count(&self.anchors, &self.filter);
            let sel = feed.list.selected().unwrap_or(0);
            if visible == 0 {
                feed.list.select(None);
            } else if sel >= visible {
                feed.list.select(Some(visible - 1));
            } else if feed.list.selected().is_none() {
                feed.list.select(Some(0));
            }
        }
    }

    fn selected_anchor(&self) -> Option<AnchorRow> {
        let View::Feed(feed) = self.stack.first()? else {
            return None;
        };
        let idx = feed.list.selected()?;
        visible_anchors(&self.anchors, &self.filter)
            .nth(idx)
            .cloned()
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.flash = Some(msg.into());
    }
}

fn visible_anchors<'a>(
    rows: &'a [AnchorRow],
    filter: &'a str,
) -> impl Iterator<Item = &'a AnchorRow> {
    let f = filter.trim().to_ascii_lowercase();
    rows.iter().filter(move |r| {
        if f.is_empty() {
            return true;
        }
        r.subject.to_ascii_lowercase().contains(&f)
            || r.sha.to_ascii_lowercase().contains(&f)
            || r.session_id
                .as_deref()
                .map(|s| s.to_ascii_lowercase().contains(&f))
                .unwrap_or(false)
            || r.intent
                .as_deref()
                .map(|s| s.to_ascii_lowercase().contains(&f))
                .unwrap_or(false)
            || r.tool
                .as_deref()
                .map(|s| s.to_ascii_lowercase().contains(&f))
                .unwrap_or(false)
    })
}

fn visible_anchor_count(rows: &[AnchorRow], filter: &str) -> usize {
    visible_anchors(rows, filter).count()
}

// ── Event loop ────────────────────────────────────────────────────────

pub(super) fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> Result<i32, String> {
    loop {
        terminal
            .draw(|frame| draw(frame, app))
            .map_err(|e| format!("tui draw: {e}"))?;

        let Some(key) = super::next_key(Duration::from_millis(200))
            .map_err(|e: io::Error| format!("key read: {e}"))?
        else {
            continue;
        };

        app.flash = None;

        if handle_key(terminal, app, key)? {
            return Ok(0);
        }
    }
}

fn handle_key(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, String> {
    if matches!(app.stack.last(), Some(View::Help)) {
        app.stack.pop();
        return Ok(false);
    }

    match app.stack.last_mut() {
        Some(View::Feed(_)) => handle_feed_key(terminal, app, key),
        Some(View::Transcript(_)) => Ok(handle_transcript_key(app, key)),
        Some(View::Search(_)) => handle_search_key(app, key),
        Some(View::Picker(_)) => handle_picker_key(terminal, app, key),
        Some(View::Help) | None => Ok(false),
    }
}

// ── Feed view keys ────────────────────────────────────────────────────

fn handle_feed_key(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, String> {
    let filter_open = matches!(
        app.stack.last(),
        Some(View::Feed(FeedState {
            filter_input_open: true,
            ..
        }))
    );
    if filter_open {
        if let Some(View::Feed(feed)) = app.stack.last_mut() {
            match key.code {
                KeyCode::Esc => {
                    feed.filter_input_open = false;
                    app.filter.clear();
                }
                KeyCode::Enter => {
                    feed.filter_input_open = false;
                }
                KeyCode::Backspace => {
                    app.filter.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.filter.push(c);
                }
                _ => {}
            }
            let total = visible_anchor_count(&app.anchors, &app.filter);
            if total == 0 {
                feed.list.select(None);
            } else {
                feed.list.select(Some(0));
            }
        }
        return Ok(false);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('/'))
    {
        app.stack.push(View::Search(SearchState {
            query: String::new(),
            global: false,
            results: Vec::new(),
            list: ListState::default(),
            running: false,
        }));
        return Ok(false);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Esc => {
            if app.filter.is_empty() {
                return Ok(true);
            }
            app.filter.clear();
            select_first_visible(app);
            app.flash("filter cleared");
        }
        KeyCode::Char('?') => {
            app.stack.push(View::Help);
        }
        KeyCode::Char('/') => {
            if let Some(View::Feed(feed)) = app.stack.last_mut() {
                feed.filter_input_open = true;
                app.filter.clear();
            }
        }
        KeyCode::Char('t') => {
            app.time_window = app.time_window.cycle();
            app.reload_anchors();
            app.flash(format!("time window: {}", app.time_window.label()));
        }
        KeyCode::Char('r') => {
            app.reload_anchors();
            app.flash("reloaded");
        }
        KeyCode::Char('e') => toggle_enabled(app),
        KeyCode::Char('c') => continue_selected_memory(terminal, app)?,
        KeyCode::Char('d') => {
            if let Some(anchor) = app.selected_anchor() {
                if anchor.kind == MemoryKind::Anchor {
                    suspend_and_run(terminal, || run_git_show(&app.root, &anchor.sha))?;
                } else {
                    app.flash("this point has no git commit yet; press enter to inspect memory");
                }
            }
        }
        KeyCode::Char('b') => {
            if app
                .selected_anchor()
                .map(|a| a.kind == MemoryKind::Anchor)
                .unwrap_or(false)
            {
                open_blame_picker(app)
            } else {
                app.flash("blame is available after this anchor is committed");
            }
        }
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::PageUp => move_selection(app, -10),
        KeyCode::PageDown => move_selection(app, 10),
        KeyCode::Char('g') => move_to(app, 0),
        KeyCode::Char('G') => move_to(app, usize::MAX),
        KeyCode::Enter | KeyCode::Char('s') => open_selected_memory(app),
        _ => {}
    }
    Ok(false)
}

fn select_first_visible(app: &mut App) {
    if let Some(View::Feed(feed)) = app.stack.first_mut() {
        if visible_anchor_count(&app.anchors, &app.filter) == 0 {
            feed.list.select(None);
        } else {
            feed.list.select(Some(0));
        }
    }
}

// ── Transcript view keys ──────────────────────────────────────────────

fn handle_transcript_key(app: &mut App, key: KeyEvent) -> bool {
    // Filter-input sub-mode absorbs keys into filter buffer.
    let filter_open = matches!(
        app.stack.last(),
        Some(View::Transcript(TranscriptState {
            filter_open: true,
            ..
        }))
    );
    if filter_open {
        if let Some(View::Transcript(ts)) = app.stack.last_mut() {
            match key.code {
                KeyCode::Esc => {
                    ts.filter_open = false;
                    ts.filter.clear();
                    ts.match_lines.clear();
                    ts.match_cursor = 0;
                }
                KeyCode::Enter => {
                    ts.filter_open = false;
                    if !ts.match_lines.is_empty() {
                        ts.scroll = ts.match_lines[0] as u16;
                        ts.match_cursor = 0;
                    }
                }
                KeyCode::Backspace => {
                    ts.filter.pop();
                    recompute_transcript_matches(ts);
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    ts.filter.push(c);
                    recompute_transcript_matches(ts);
                }
                _ => {}
            }
        }
        return false;
    }

    if let Some(View::Transcript(ts)) = app.stack.last_mut() {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc | KeyCode::Backspace => {
                app.stack.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ts.scroll = ts.scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ts.scroll = ts.scroll.saturating_add(1);
            }
            KeyCode::PageUp => {
                ts.scroll = ts.scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                ts.scroll = ts.scroll.saturating_add(10);
            }
            KeyCode::Char('g') => {
                ts.scroll = 0;
            }
            KeyCode::Char('G') => {
                ts.scroll = ts.lines.len().saturating_sub(1) as u16;
            }
            KeyCode::Char('[') => {
                if !ts.sessions.is_empty() {
                    ts.idx = if ts.idx == 0 {
                        ts.sessions.len() - 1
                    } else {
                        ts.idx - 1
                    };
                    reload_transcript(ts);
                }
            }
            KeyCode::Char(']') => {
                if !ts.sessions.is_empty() {
                    ts.idx = (ts.idx + 1) % ts.sessions.len();
                    reload_transcript(ts);
                }
            }
            KeyCode::Char('/') => {
                ts.filter_open = true;
                ts.filter.clear();
                ts.match_lines.clear();
                ts.match_cursor = 0;
            }
            KeyCode::Char('n') => jump_to_match(ts, 1),
            KeyCode::Char('N') => jump_to_match(ts, -1),
            _ => {}
        }
    }
    false
}

fn recompute_transcript_matches(ts: &mut TranscriptState) {
    ts.match_lines.clear();
    ts.match_cursor = 0;
    let needle = ts.filter.to_ascii_lowercase();
    if needle.is_empty() {
        return;
    }
    for (i, line) in ts.lines.iter().enumerate() {
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        if text.to_ascii_lowercase().contains(&needle) {
            ts.match_lines.push(i);
        }
    }
    if let Some(first) = ts.match_lines.first() {
        ts.scroll = *first as u16;
    }
}

fn jump_to_match(ts: &mut TranscriptState, delta: i32) {
    if ts.match_lines.is_empty() {
        return;
    }
    let n = ts.match_lines.len() as i32;
    let cur = ts.match_cursor as i32;
    let next = (cur + delta).rem_euclid(n) as usize;
    ts.match_cursor = next;
    ts.scroll = ts.match_lines[next] as u16;
}

fn reload_transcript(ts: &mut TranscriptState) {
    let session = &ts.sessions[ts.idx];
    ts.lines = load_transcript_lines(&ts.project_path, session);
    ts.scroll = 0;
    ts.match_lines.clear();
    ts.match_cursor = 0;
    if !ts.filter.is_empty() {
        recompute_transcript_matches(ts);
    }
}

// ── Search view keys ──────────────────────────────────────────────────

fn handle_search_key(app: &mut App, key: KeyEvent) -> Result<bool, String> {
    // Handle nav/action keys before consuming printable keys into the query.
    let Some(View::Search(state)) = app.stack.last_mut() else {
        return Ok(false);
    };

    match key.code {
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
        KeyCode::Esc => {
            app.stack.pop();
            return Ok(false);
        }
        KeyCode::Tab => {
            state.global = !state.global;
            if !state.query.trim().is_empty() {
                run_search(app);
            }
            return Ok(false);
        }
        KeyCode::Enter => {
            // If results exist and a row is selected, drill into it;
            // otherwise run/re-run the query.
            if !state.results.is_empty() && state.list.selected().is_some() {
                open_search_hit(app);
            } else if !state.query.trim().is_empty() {
                run_search(app);
            }
            return Ok(false);
        }
        KeyCode::Backspace => {
            state.query.pop();
            return Ok(false);
        }
        KeyCode::Up => {
            nav_search(state, -1);
            return Ok(false);
        }
        KeyCode::Down => {
            nav_search(state, 1);
            return Ok(false);
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.query.push(c);
            return Ok(false);
        }
        _ => {}
    }
    Ok(false)
}

fn nav_search(state: &mut SearchState, delta: i32) {
    if state.results.is_empty() {
        return;
    }
    let cur = state.list.selected().unwrap_or(0) as i32;
    let n = state.results.len() as i32;
    let next = (cur + delta).clamp(0, n - 1);
    state.list.select(Some(next as usize));
}

fn run_search(app: &mut App) {
    let Some(View::Search(state)) = app.stack.last_mut() else {
        return;
    };
    state.running = true;
    let query = state.query.clone();
    let scope = if state.global {
        crate::commands::search::Scope::Global
    } else {
        crate::commands::search::Scope::CurrentRepo(app.root.clone())
    };
    let opts = crate::commands::search::Options {
        source: Some(crate::commands::search::Source::Local),
        since: None,
        scope,
        tool: None,
        limit: 100,
    };
    let hits = crate::commands::search::collect_local(&app.cfg, &query, &opts).unwrap_or_default();
    if let Some(View::Search(state)) = app.stack.last_mut() {
        state.running = false;
        state.results = hits;
        if state.results.is_empty() {
            state.list.select(None);
        } else {
            state.list.select(Some(0));
        }
    }
}

/// Drill into a selected search hit — open transcript if we have a session_id,
/// otherwise focus on the anchor by jumping to it in the feed (if same project).
fn open_search_hit(app: &mut App) {
    let (session_id, source, project_id, anchor_sha) = {
        let Some(View::Search(state)) = app.stack.last() else {
            return;
        };
        let Some(idx) = state.list.selected() else {
            return;
        };
        let Some(hit) = state.results.get(idx) else {
            return;
        };
        (
            hit.session_id.clone(),
            hit.tool.clone(),
            hit.project_id.clone(),
            hit.anchor_sha.clone(),
        )
    };

    // Resolve target project_path from the DB (may differ from current repo).
    let project_path = match lookup_project_path(&project_id) {
        Some(p) => p,
        None => app.root.clone(),
    };

    // Session hit → open its transcript directly.
    if let (Some(sid), Some(src)) = (session_id, source) {
        let link = SessionLink {
            session_id: sid,
            source: src,
            model: None,
            tokens: 0,
        };
        let lines = load_transcript_lines(&project_path, &link);
        app.stack.push(View::Transcript(TranscriptState {
            sessions: vec![link],
            idx: 0,
            project_path,
            lines,
            scroll: 0,
            filter: String::new(),
            filter_open: false,
            match_lines: Vec::new(),
            match_cursor: 0,
        }));
        return;
    }

    // Anchor hit → load its sessions and open the first transcript.
    if let Some(sha) = anchor_sha {
        let sessions = load_sessions_for_anchor(&project_path, &sha);
        if sessions.is_empty() {
            app.flash("no sessions linked to this anchor");
            return;
        }
        let lines = load_transcript_lines(&project_path, &sessions[0]);
        app.stack.push(View::Transcript(TranscriptState {
            sessions,
            idx: 0,
            project_path,
            lines,
            scroll: 0,
            filter: String::new(),
            filter_open: false,
            match_lines: Vec::new(),
            match_cursor: 0,
        }));
    }
}

// ── Picker view keys ──────────────────────────────────────────────────

fn handle_picker_key(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, String> {
    // Navigation first.
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.stack.pop();
            return Ok(false);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(View::Picker(p)) = app.stack.last_mut() {
                picker_move(p, -1);
            }
            return Ok(false);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(View::Picker(p)) = app.stack.last_mut() {
                picker_move(p, 1);
            }
            return Ok(false);
        }
        KeyCode::Enter => {
            // Resolve action and pop.
            let action = match app.stack.last_mut() {
                Some(View::Picker(p)) => resolve_picker_action(p),
                _ => return Ok(false),
            };
            app.stack.pop();
            apply_picker_action(terminal, app, action)?;
            return Ok(false);
        }
        _ => {}
    }
    Ok(false)
}

fn picker_move(p: &mut PickerState, delta: i32) {
    let n = p.len() as i32;
    if n == 0 {
        return;
    }
    let cur = p.list.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, n - 1);
    p.list.select(Some(next as usize));
}

fn resolve_picker_action(p: &PickerState) -> PickerAction {
    let Some(idx) = p.list.selected() else {
        return PickerAction::Noop;
    };
    match &p.kind {
        PickerKind::Session {
            sessions,
            project_path,
        } => sessions
            .get(idx)
            .map(|s| PickerAction::OpenSession {
                session: s.clone(),
                project_path: project_path.clone(),
                siblings: sessions.clone(),
                idx,
            })
            .unwrap_or(PickerAction::Noop),
        PickerKind::BlameFile { files, sha, root } => files
            .get(idx)
            .map(|f| PickerAction::Blame {
                root: root.clone(),
                file: f.clone(),
                sha: sha.clone(),
            })
            .unwrap_or(PickerAction::Noop),
    }
}

fn apply_picker_action(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    action: PickerAction,
) -> Result<(), String> {
    match action {
        PickerAction::OpenSession {
            session,
            project_path,
            siblings,
            idx,
        } => {
            let lines = load_transcript_lines(&project_path, &session);
            app.stack.push(View::Transcript(TranscriptState {
                sessions: siblings,
                idx,
                project_path,
                lines,
                scroll: 0,
                filter: String::new(),
                filter_open: false,
                match_lines: Vec::new(),
                match_cursor: 0,
            }));
        }
        PickerAction::Blame { root, file, sha } => {
            suspend_and_run(terminal, || run_oobo_blame(&root, &file, &sha))?;
        }
        PickerAction::Noop => {}
    }
    Ok(())
}

// ── Actions ───────────────────────────────────────────────────────────

fn move_selection(app: &mut App, delta: i32) {
    let total = visible_anchor_count(&app.anchors, &app.filter) as i32;
    if total == 0 {
        return;
    }
    if let Some(View::Feed(feed)) = app.stack.last_mut() {
        let cur = feed.list.selected().unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, total - 1);
        feed.list.select(Some(next as usize));
    }
}

fn move_to(app: &mut App, idx: usize) {
    let total = visible_anchor_count(&app.anchors, &app.filter);
    if total == 0 {
        return;
    }
    if let Some(View::Feed(feed)) = app.stack.last_mut() {
        feed.list.select(Some(idx.min(total - 1)));
    }
}

fn open_selected_memory(app: &mut App) {
    let Some(anchor) = app.selected_anchor() else {
        return;
    };
    match anchor.kind {
        MemoryKind::Anchor => open_sessions_for_anchor_row(app, &anchor),
        MemoryKind::ShadowAnchor => open_shadow_memory(app, &anchor),
    }
}

fn continue_selected_memory(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> Result<(), String> {
    let Some(anchor) = app.selected_anchor() else {
        return Ok(());
    };
    if anchor.kind != MemoryKind::ShadowAnchor {
        app.flash("continue is for working memory");
        return Ok(());
    }
    if worktree_dirty(&app.root) {
        app.dirty = true;
        app.flash(format!(
            "worktree dirty; run `anchor from turn {} --load --force`",
            short_sha(&anchor.sha)
        ));
        return Ok(());
    }

    let root = app.root.clone();
    let turn_id = anchor.sha.clone();
    suspend_and_run(terminal, || run_oobo_from_turn(&root, &turn_id))?;
    app.reload_anchors();
    app.flash("loaded working memory");
    Ok(())
}

/// Open transcript for the selected anchor. If it has a single session,
/// open it directly. If multiple, push a session picker.
fn open_sessions_for_anchor_row(app: &mut App, anchor: &AnchorRow) {
    let sessions = load_sessions_for_anchor(&app.root, &anchor.sha);
    if sessions.is_empty() {
        app.flash("no sessions linked to this anchor");
        return;
    }
    if sessions.len() == 1 {
        let session = sessions.into_iter().next().unwrap();
        let lines = load_transcript_lines(&app.root, &session);
        app.stack.push(View::Transcript(TranscriptState {
            sessions: vec![session],
            idx: 0,
            project_path: app.root.clone(),
            lines,
            scroll: 0,
            filter: String::new(),
            filter_open: false,
            match_lines: Vec::new(),
            match_cursor: 0,
        }));
        return;
    }

    let mut list = ListState::default();
    list.select(Some(0));
    app.stack.push(View::Picker(PickerState {
        title: format!("sessions for {}", short_sha(&anchor.sha)),
        list,
        kind: PickerKind::Session {
            sessions,
            project_path: app.root.clone(),
        },
    }));
}

fn open_shadow_memory(app: &mut App, shadow: &AnchorRow) {
    let Some(session_id) = shadow.session_id.clone() else {
        app.flash("no session captured for this anchor");
        return;
    };
    let source = shadow.tool.clone().unwrap_or_else(|| "unknown".to_string());
    let session = SessionLink {
        session_id,
        source,
        model: None,
        tokens: 0,
    };
    let mut lines = load_transcript_lines(&app.root, &session);
    if lines.is_empty() {
        lines = vec![
            Line::from(Span::styled(
                "anchor memory",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!("prompt: {}", shadow.subject)),
            Line::from(format!("files:  {}", shadow.files)),
            Line::from(format!("tools:  {}", shadow.tool_calls)),
            Line::from(""),
            Line::from(Span::styled(
                "Use `anchor from turn <id> --load` to restore this point.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
    }
    app.stack.push(View::Transcript(TranscriptState {
        sessions: vec![session],
        idx: 0,
        project_path: app.root.clone(),
        lines,
        scroll: 0,
        filter: String::new(),
        filter_open: false,
        match_lines: Vec::new(),
        match_cursor: 0,
    }));
}

/// Build a file picker for `b` — blame one of the touched files at the
/// selected anchor. Skips the picker if only one file.
fn open_blame_picker(app: &mut App) {
    let Some(anchor) = app.selected_anchor() else {
        return;
    };
    let files = touched_files_for(&app.root, &anchor.sha);
    if files.is_empty() {
        app.flash("no files touched in this anchor");
        return;
    }
    if files.len() == 1 {
        let file = files.into_iter().next().unwrap();
        // Stash and invoke after returning true, but we don't have the terminal
        // here; so set a deferred flash and return; actual suspend happens
        // through the picker action path for uniformity.
        let sha = anchor.sha.clone();
        let mut list = ListState::default();
        list.select(Some(0));
        app.stack.push(View::Picker(PickerState {
            title: format!("blame at {}  (press enter)", short_sha(&sha)),
            list,
            kind: PickerKind::BlameFile {
                files: vec![file],
                sha,
                root: app.root.clone(),
            },
        }));
        return;
    }

    let mut list = ListState::default();
    list.select(Some(0));
    app.stack.push(View::Picker(PickerState {
        title: format!("blame — pick a file (at {})", short_sha(&anchor.sha)),
        list,
        kind: PickerKind::BlameFile {
            files,
            sha: anchor.sha.clone(),
            root: app.root.clone(),
        },
    }));
}

fn toggle_enabled(app: &mut App) {
    let next_enabled = !crate::project_config::is_enabled(&app.root);
    if crate::project_config::set_enabled(&app.root, &app.project_id, next_enabled).is_err() {
        app.flash("cannot update settings");
        return;
    }
    app.enabled = crate::project_config::is_enabled(&app.root);
    app.flash(if app.enabled {
        "tracking enabled"
    } else {
        "tracking disabled"
    });
}

// ── External commands (suspend/restore TUI) ──────────────────────────

fn suspend_and_run<F>(terminal: &mut ratatui::DefaultTerminal, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    super::restore();
    let result = f();
    *terminal = super::init().map_err(|e| format!("tui init: {e}"))?;
    terminal.clear().ok();
    result
}

fn run_git_show(root: &str, sha: &str) -> Result<(), String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let status = std::process::Command::new(&git)
        .args(["show", "--color=always", sha])
        .current_dir(root)
        .env("GIT_PAGER", "less -R")
        .status()
        .map_err(|e| format!("spawn git show: {e}"))?;
    if !status.success() {
        return Err(format!("git show exited {status}"));
    }
    Ok(())
}

fn run_oobo_blame(root: &str, file: &str, sha: &str) -> Result<(), String> {
    let oobo = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("anchor"));
    let status = std::process::Command::new(oobo)
        .args(["blame", file, sha])
        .current_dir(root)
        .status()
        .map_err(|e| format!("spawn anchor blame: {e}"))?;
    if !status.success() {
        return Err(format!("anchor blame exited {status}"));
    }
    println!("\npress enter to return...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
    Ok(())
}

fn run_oobo_from_turn(root: &str, turn_id: &str) -> Result<(), String> {
    let oobo = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("anchor"));
    let status = std::process::Command::new(oobo)
        .args(["from", "turn", turn_id, "--load"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("spawn anchor from: {e}"))?;
    if !status.success() {
        return Err(format!("anchor from exited {status}"));
    }
    println!("\npress enter to return...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
    Ok(())
}

// ── DB loaders ────────────────────────────────────────────────────────

fn load_anchors(
    cfg: &Config,
    project_root: &str,
    limit: usize,
    window: TimeWindow,
) -> Result<Vec<AnchorRow>, String> {
    let n = limit.max(1);
    let log = crate::git::proxy::run_git_capture_in(
        cfg,
        &["log", &format!("-{}", n), "--format=%H|||%s|||%ct"],
        Some(project_root),
    )
    .unwrap_or_default();

    let (all_anchors, all_links) =
        crate::git::anchor_cache::load_anchors_cached(project_root);
    let anchor_map: HashMap<String, &crate::core::anchor::Anchor> = all_anchors
        .iter()
        .map(|a| (a.commit_hash.clone(), a))
        .collect();

    let cutoff = window.cutoff().unwrap_or(0);
    let mut rows: Vec<AnchorRow> = Vec::new();
    for line in log.lines() {
        let parts: Vec<&str> = line.splitn(3, "|||").collect();
        if parts.len() < 3 {
            continue;
        }
        let sha = parts[0].to_string();
        let subject_git = parts[1].to_string();
        let ts: i64 = parts[2].parse().unwrap_or(0);
        if ts < cutoff {
            continue;
        }

        let (anchor_opt, summary) = if let Some(anchor) = anchor_map.get(&sha) {
            let links = all_links.get(&sha).cloned().unwrap_or_default();
            let s = summarize_from_links(&links);
            (Some((*anchor).clone()), s)
        } else {
            (
                None,
                AnchorSummary {
                    tool: None,
                    tokens: None,
                    count: 0,
                },
            )
        };

        let intent = anchor_opt.as_ref().and_then(|a| a.intent.clone());
        let subject = intent.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
            if subject_git.is_empty() {
                "(no subject)".to_string()
            } else {
                subject_git.clone()
            }
        });

        rows.push(AnchorRow {
            kind: MemoryKind::Anchor,
            sha,
            timestamp: ts,
            subject,
            intent,
            tool: summary.tool,
            tokens: summary.tokens,
            session_count: summary.count,
            files: 0,
            tool_calls: 0,
            turn_index: None,
            session_id: None,
            parent_anchor: None,
        });
    }
    let parents = build_shadow_parents_from_cached(&all_anchors);
    rows.extend(load_shadow_rows(project_root, window, &parents));
    sort_memory_rows(&mut rows);
    rows.truncate(limit);
    Ok(rows)
}

fn load_shadow_rows(
    project_root: &str,
    window: TimeWindow,
    parents: &HashMap<String, String>,
) -> Vec<AnchorRow> {
    let cutoff = window.cutoff().unwrap_or(0);
    crate::git::turns::list_turn_snapshots(project_root)
        .into_iter()
        .filter_map(|turn| {
            let ts = turn.ended_at.or(turn.started_at).unwrap_or(turn.created_at);
            if ts < cutoff {
                return None;
            }
            Some(AnchorRow {
                kind: MemoryKind::ShadowAnchor,
                sha: turn.id.clone(),
                timestamp: ts,
                subject: shadow_subject(&turn),
                intent: None,
                tool: Some(turn.source.clone()),
                tokens: None,
                session_count: 1,
                files: turn_file_count(&turn),
                tool_calls: turn.memory.tool_calls.len(),
                turn_index: Some(turn.turn_index),
                session_id: Some(turn.session_id.clone()),
                parent_anchor: parents.get(&turn.id).cloned(),
            })
        })
        .collect()
}

fn shadow_subject(turn: &crate::core::turn::TurnSnapshot) -> String {
    for event in &turn.memory.hook_events {
        let Some(payload) = event.payload.as_ref() else {
            continue;
        };
        for key in ["prompt", "message", "text", "input"] {
            if let Some(value) = payload.get(key).and_then(|v| v.as_str()) {
                let value = value.lines().next().unwrap_or(value).trim();
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    format!("anchor #{}", turn.turn_index)
}

fn turn_file_count(turn: &crate::core::turn::TurnSnapshot) -> usize {
    let mut files = std::collections::HashSet::new();
    for call in &turn.memory.tool_calls {
        if let Some(input) = call.input.as_ref() {
            collect_file_paths_from_value(input, &mut files);
        }
    }
    for event in &turn.memory.hook_events {
        if let Some(payload) = event.payload.as_ref() {
            collect_file_paths_from_value(payload, &mut files);
        }
    }
    if files.is_empty() {
        turn.files.len()
    } else {
        files.len()
    }
}

fn collect_file_paths_from_value(
    value: &serde_json::Value,
    files: &mut std::collections::HashSet<String>,
) {
    for key in ["file_path", "path"] {
        if let Some(path) = value.get(key).and_then(|v| v.as_str()) {
            push_counted_file(path, files);
        }
    }
    for key in ["modified_files", "files", "file_paths"] {
        if let Some(items) = value.get(key).and_then(|v| v.as_array()) {
            for item in items {
                if let Some(path) = item.as_str() {
                    push_counted_file(path, files);
                }
            }
        }
    }
    if let Some(input) = value.get("tool_input") {
        collect_file_paths_from_value(input, files);
    }
}

fn push_counted_file(path: &str, files: &mut std::collections::HashSet<String>) {
    if path.is_empty() || path == "." || path.ends_with('/') {
        return;
    }
    files.insert(path.to_string());
}

fn build_shadow_parents_from_cached(
    all_anchors: &[crate::core::anchor::Anchor],
) -> HashMap<String, String> {
    let mut parents = HashMap::new();
    for anchor in all_anchors {
        for turn in &anchor.turns {
            parents
                .entry(turn.id.clone())
                .or_insert_with(|| anchor.commit_hash.clone());
        }
    }
    parents
}

fn sort_memory_rows(rows: &mut [AnchorRow]) {
    rows.sort_by(|a, b| {
        if a.parent_anchor.as_deref() == Some(b.sha.as_str()) {
            return std::cmp::Ordering::Greater;
        }
        if b.parent_anchor.as_deref() == Some(a.sha.as_str()) {
            return std::cmp::Ordering::Less;
        }
        if a.parent_anchor.is_some() && a.parent_anchor == b.parent_anchor {
            return a
                .turn_index
                .cmp(&b.turn_index)
                .then_with(|| a.sha.cmp(&b.sha));
        }
        b.timestamp
            .cmp(&a.timestamp)
            .then_with(|| b.sha.cmp(&a.sha))
    });
}

struct AnchorSummary {
    tool: Option<String>,
    tokens: Option<i64>,
    count: usize,
}

fn summarize_from_links(links: &[crate::core::anchor::SessionLink]) -> AnchorSummary {
    if links.is_empty() {
        return AnchorSummary {
            tool: None,
            tokens: None,
            count: 0,
        };
    }
    let tool = Some(links[0].agent.clone());
    let total: i64 = links
        .iter()
        .map(|l| {
            l.input_tokens.unwrap_or(0) as i64
                + l.output_tokens.unwrap_or(0) as i64
                + l.cache_read_tokens.unwrap_or(0) as i64
                + l.cache_creation_tokens.unwrap_or(0) as i64
        })
        .sum();
    AnchorSummary {
        tool,
        tokens: if total == 0 { None } else { Some(total) },
        count: links.len(),
    }
}

fn load_sessions_for_anchor(project_root: &str, commit_hash: &str) -> Vec<SessionLink> {
    crate::git::orphan::read_session_links(project_root, commit_hash)
        .into_iter()
        .map(|l| {
            let tokens = l.input_tokens.unwrap_or(0) as i64
                + l.output_tokens.unwrap_or(0) as i64
                + l.cache_read_tokens.unwrap_or(0) as i64
                + l.cache_creation_tokens.unwrap_or(0) as i64;
            SessionLink {
                session_id: l.session_id,
                source: l.agent,
                model: l.model,
                tokens,
            }
        })
        .collect()
}

fn lookup_project_path(_project_id: &str) -> Option<String> {
    None
}

fn touched_files_for(root: &str, sha: &str) -> Vec<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let Ok(output) = std::process::Command::new(git)
        .args(["show", "--name-only", "--pretty=format:", sha])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn current_branch(root: &str) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(git)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

fn worktree_dirty(root: &str) -> bool {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let Ok(output) = std::process::Command::new(git)
        .args(["status", "--porcelain"])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
}

// ── Rendering ─────────────────────────────────────────────────────────

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    // Always draw the root view (Feed) as the base. Then, if another view is
    // on top, overlay it. This keeps the feed visible behind overlays like
    // Help and Picker.
    if let Some(first) = app.stack.first() {
        match first {
            View::Feed(feed) => draw_feed(frame, app, feed),
            View::Transcript(ts) => super::transcript::draw_transcript(frame, ts),
            View::Search(s) => draw_search(frame, app, s),
            View::Picker(_) | View::Help => {}
        }
    }

    // Draw additional views on top of the root (if any).
    if app.stack.len() > 1 {
        if let Some(top) = app.stack.last() {
            match top {
                View::Feed(_) => {} // already drawn as root
                View::Transcript(ts) => super::transcript::draw_transcript(frame, ts),
                View::Search(s) => draw_search(frame, app, s),
                View::Picker(p) => draw_picker_overlay(frame, p),
                View::Help => draw_help_overlay(frame),
            }
        }
    } else if matches!(app.stack.last(), Some(View::Help)) {
        draw_help_overlay(frame);
    }
}

fn draw_feed(frame: &mut ratatui::Frame<'_>, app: &App, feed: &FeedState) {
    let area = frame.area();
    let notice = !app.enabled;
    let constraints = if notice {
        vec![
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let (header_area, notice_area, body_area, footer_area) = if notice {
        (layout[0], Some(layout[1]), layout[2], layout[3])
    } else {
        (layout[0], None, layout[1], layout[2])
    };

    draw_header(frame, app, header_area);

    if let Some(na) = notice_area {
        draw_tracking_notice(frame, na);
    }

    if app.anchors.is_empty() {
        draw_empty_state(frame, app, body_area);
        draw_footer(frame, app, feed, footer_area);
        return;
    }
    if visible_anchor_count(&app.anchors, &app.filter) == 0 {
        draw_filtered_empty_state(frame, app, body_area);
        draw_footer(frame, app, feed, footer_area);
        return;
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
        .split(body_area);

    draw_anchor_list(frame, app, feed, body[0]);
    draw_anchor_detail(frame, app, body[1]);
    draw_footer(frame, app, feed, footer_area);
}

fn draw_tracking_notice(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let line = Line::from(vec![
        Span::styled("  tracking off", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(
            " — new sessions won't be captured on commit. press ",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled("e", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(" to enable", Style::default().fg(Color::Yellow)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_empty_state(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let hints: Vec<Line<'static>> = if !app.enabled {
        vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                format!("  anchor is not tracking \"{}\"", app.project_name),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  anchor captures AI sessions and links them to your commits.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  enable tracking to start building memory for this project.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("  · press ", Style::default().fg(Color::Gray)),
                Span::styled("e", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(" to enable tracking now", Style::default().fg(Color::Gray)),
            ]),
            Line::from(Span::styled(
                "  · or run:  anchor enable", Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  press ? for keybindings · q to quit",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                format!("  no memory yet for \"{}\"", app.project_name),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  anchor captures working memory as you go and makes it durable when you commit.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  · make a commit:   anchor commit -m \"your message\"",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "  · (re)install git alias:  anchor alias install",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "  · index existing sessions: anchor setup",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  press ? for keybindings · q to quit",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    };
    frame.render_widget(Paragraph::new(hints).wrap(Wrap { trim: false }), area);
}

fn draw_filtered_empty_state(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  no memory items match this filter",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  filter ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("\"{}\"", app.filter),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  press / to search again · esc to clear · ? for keybindings",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_header(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let total_items = app.anchors.len();
    let filtered = visible_anchor_count(&app.anchors, &app.filter);
    let committed = app
        .anchors
        .iter()
        .filter(|r| r.kind == MemoryKind::Anchor)
        .count();
    let working = total_items.saturating_sub(committed);
    let count_str = if app.filter.is_empty() {
        format!("{committed} committed  {working} working")
    } else {
        format!("{filtered}/{total_items} memory items")
    };
    let branch = app.branch.as_deref().unwrap_or("detached");
    let dirty_label = if app.dirty { "dirty" } else { "clean" };
    let dirty_style = if app.dirty {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                "  anchor",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" / ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.project_name.clone(), Style::default().fg(Color::Gray)),
            Span::styled("  memory", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            chip("branch", branch, Color::Cyan),
            Span::styled("  ", Style::default()),
            chip(
                "tree",
                dirty_label,
                if app.dirty {
                    Color::Yellow
                } else {
                    Color::DarkGray
                },
            ),
            Span::styled("  ", Style::default()),
            chip("anchors", &app.anchor_remote, Color::Magenta),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(count_str, Style::default().fg(Color::Gray)),
            Span::styled("   ", Style::default()),
            Span::styled(
                format!("window {}", app.time_window.label()),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("   ", Style::default()),
            Span::styled(
                if app.enabled {
                    "tracking on"
                } else {
                    "tracking off"
                },
                if app.enabled {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
            Span::styled("   ", Style::default()),
            Span::styled(dirty_label, dirty_style),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Left), area);
}

fn chip(label: &str, value: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!("{label} {value}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn draw_anchor_list(frame: &mut ratatui::Frame<'_>, app: &App, feed: &FeedState, area: Rect) {
    // Keep the feed quiet: state is expressed by timeline rail + typography.
    let total_w = area.width.saturating_sub(2) as usize;
    let meta_w = 18usize;
    let prefix_w = 9usize;
    let subject_w = total_w.saturating_sub(meta_w + prefix_w).max(12);

    let items: Vec<ListItem> = visible_anchors(&app.anchors, &app.filter)
        .map(|r| {
            let when = relative_time(r.timestamp);
            let (rail, dot, dot_style, subject_style) = match r.kind {
                MemoryKind::Anchor => (
                    "│",
                    "●",
                    Style::default().fg(Color::Green),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                MemoryKind::ShadowAnchor => (
                    if r.parent_anchor.is_some() {
                        "╰"
                    } else {
                        " "
                    },
                    "·",
                    Style::default().fg(Color::DarkGray),
                    Style::default().fg(Color::DarkGray),
                ),
            };

            let meta = match r.kind {
                MemoryKind::Anchor => {
                    let tok_str = r
                        .tokens
                        .filter(|t| *t > 0)
                        .map(super::format_tokens)
                        .unwrap_or_else(|| "-".into());
                    let sessions = if r.session_count > 0 {
                        format!("{}s", r.session_count)
                    } else {
                        "-".to_string()
                    };
                    format!("{tok_str:>7} {sessions:>3}")
                }
                MemoryKind::ShadowAnchor => {
                    let turn = r
                        .turn_index
                        .map(|idx| format!("#{idx}"))
                        .unwrap_or_else(|| "#-".to_string());
                    format!("{:>3}f {:>3}t {turn:>4}", r.files, r.tool_calls)
                }
            };

            let subject_raw = if r.subject.is_empty() {
                "(no subject)".to_string()
            } else {
                r.subject.clone()
            };
            let subject = pad_or_truncate(&subject_raw, subject_w);

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {rail} "), Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{dot} "), dot_style),
                Span::styled(format!("{when:<5} "), Style::default().fg(Color::DarkGray)),
                Span::styled(subject, subject_style),
                Span::styled(format!("  {meta}"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = feed.list;
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_anchor_detail(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let Some(anchor) = app.selected_anchor() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  (no anchor selected)",
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    };

    if anchor.kind == MemoryKind::ShadowAnchor {
        draw_shadow_detail(frame, app, &anchor, area);
        return;
    }

    let sessions = load_sessions_for_anchor(&app.root, &anchor.sha);
    let files = touched_files_for(&app.root, &anchor.sha);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("  ● ", Style::default().fg(Color::Green)),
        Span::styled(
            format!("  {}", short_sha(&anchor.sha)),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} ago", relative_time(anchor.timestamp)),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "  committed memory",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", anchor.subject),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if let Some(intent) = anchor.intent.as_deref() {
        if !intent.is_empty() && intent != anchor.subject {
            lines.push(Line::from(Span::styled(
                format!("  {intent}"),
                Style::default().fg(Color::Gray),
            )));
        }
    }
    lines.push(Line::from(""));

    let tool = anchor.tool.as_deref().unwrap_or("-");
    let tokens = anchor
        .tokens
        .map(super::format_tokens)
        .unwrap_or_else(|| "-".into());
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        chip("tool", tool, Color::Cyan),
        Span::styled("  ", Style::default()),
        chip("tokens", &tokens, Color::Green),
        Span::styled("  ", Style::default()),
        chip(
            "sessions",
            &anchor.session_count.to_string(),
            Color::Magenta,
        ),
    ]));

    if !sessions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  sessions",
            Style::default().fg(Color::DarkGray),
        )));
        for s in &sessions {
            let short_sid: String = s.session_id.chars().take(8).collect();
            let model = s.model.clone().unwrap_or_else(|| "-".into());
            let tok = if s.tokens > 0 {
                super::format_tokens(s.tokens)
            } else {
                "-".into()
            };
            lines.push(Line::from(vec![
                Span::styled("    · ", Style::default().fg(Color::Green)),
                Span::raw(short_sid),
                Span::styled(
                    format!("  {}  {}  {tok}", s.source, model),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    if !files.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  files ({})", files.len()),
            Style::default().fg(Color::DarkGray),
        )));
        for f in files.iter().take(12) {
            lines.push(Line::from(Span::raw(format!("    {f}"))));
        }
        if files.len() > 12 {
            lines.push(Line::from(Span::styled(
                format!("    … {} more", files.len() - 12),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  actions",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(vec![
        Span::styled("    enter", Style::default().fg(Color::White)),
        Span::styled(" memory", Style::default().fg(Color::DarkGray)),
        Span::styled("   d", Style::default().fg(Color::White)),
        Span::styled(" diff", Style::default().fg(Color::DarkGray)),
        Span::styled("   b", Style::default().fg(Color::White)),
        Span::styled(" blame", Style::default().fg(Color::DarkGray)),
    ]));

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn draw_shadow_detail(frame: &mut ratatui::Frame<'_>, app: &App, shadow: &AnchorRow, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("  · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("  {}", short_sha(&shadow.sha)),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} ago", relative_time(shadow.timestamp)),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "  working memory",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", shadow.subject),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        chip(
            "source",
            &shadow.tool.clone().unwrap_or_else(|| "-".to_string()),
            Color::Cyan,
        ),
        Span::styled("  ", Style::default()),
        chip("files", &shadow.files.to_string(), Color::Gray),
        Span::styled("  ", Style::default()),
        chip("tools", &shadow.tool_calls.to_string(), Color::Magenta),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  session ", Style::default().fg(Color::DarkGray)),
        Span::raw(
            shadow
                .session_id
                .as_deref()
                .map(short_session)
                .unwrap_or_else(|| "-".to_string()),
        ),
    ]));
    if let Some(parent) = shadow.parent_anchor.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("  follows ", Style::default().fg(Color::DarkGray)),
            Span::raw(short_sha(parent)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  continue from here",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(vec![
        Span::styled("    anchor from turn ", Style::default().fg(Color::DarkGray)),
        Span::styled(shadow.sha.clone(), Style::default().fg(Color::Gray)),
        Span::styled(" --load", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(""));
    if app.dirty {
        lines.push(Line::from(Span::styled(
            "  worktree is dirty; loading this point will require --force",
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled("  enter", Style::default().fg(Color::White)),
        Span::styled(
            " opens captured memory",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("   c", Style::default().fg(Color::White)),
        Span::styled(" continues from here", Style::default().fg(Color::DarkGray)),
    ]));

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn short_session(session_id: &str) -> String {
    session_id.chars().take(18).collect()
}

fn draw_footer(frame: &mut ratatui::Frame<'_>, app: &App, feed: &FeedState, area: Rect) {
    let content: Line<'static> = if feed.filter_input_open {
        Line::from(vec![
            Span::styled(
                " filter: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.filter.clone()),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::styled(
                "   (enter to apply · esc to clear)",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if let Some(flash) = &app.flash {
        Line::from(Span::styled(
            format!(" {flash} "),
            Style::default().fg(Color::Yellow),
        ))
    } else if !app.filter.is_empty() {
        Line::from(vec![
            Span::styled(" filter ", Style::default().fg(Color::Yellow)),
            Span::raw(app.filter.clone()),
            Span::styled(
                " · / edit · esc clear · ? help ",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        match app.selected_anchor().map(|row| row.kind) {
            Some(MemoryKind::Anchor) => Line::from(vec![
                Span::styled(" ↑↓", Style::default().fg(Color::White)),
                Span::styled(" move  ", Style::default().fg(Color::DarkGray)),
                Span::styled("enter", Style::default().fg(Color::White)),
                Span::styled(" memory  ", Style::default().fg(Color::DarkGray)),
                Span::styled("d", Style::default().fg(Color::White)),
                Span::styled(" diff  ", Style::default().fg(Color::DarkGray)),
                Span::styled("b", Style::default().fg(Color::White)),
                Span::styled(" blame  ", Style::default().fg(Color::DarkGray)),
                Span::styled("/", Style::default().fg(Color::White)),
                Span::styled(" filter  ", Style::default().fg(Color::DarkGray)),
                Span::styled("?", Style::default().fg(Color::White)),
                Span::styled(" help  ", Style::default().fg(Color::DarkGray)),
                Span::styled("q", Style::default().fg(Color::White)),
                Span::styled(" quit", Style::default().fg(Color::DarkGray)),
            ]),
            Some(MemoryKind::ShadowAnchor) => Line::from(vec![
                Span::styled(" ↑↓", Style::default().fg(Color::White)),
                Span::styled(" move  ", Style::default().fg(Color::DarkGray)),
                Span::styled("enter", Style::default().fg(Color::White)),
                Span::styled(" captured memory  ", Style::default().fg(Color::DarkGray)),
                Span::styled("c", Style::default().fg(Color::White)),
                Span::styled(" continue  ", Style::default().fg(Color::DarkGray)),
                Span::styled("/", Style::default().fg(Color::White)),
                Span::styled(" filter  ", Style::default().fg(Color::DarkGray)),
                Span::styled("t", Style::default().fg(Color::White)),
                Span::styled(" time  ", Style::default().fg(Color::DarkGray)),
                Span::styled("r", Style::default().fg(Color::White)),
                Span::styled(" reload  ", Style::default().fg(Color::DarkGray)),
                Span::styled("?", Style::default().fg(Color::White)),
                Span::styled(" help  ", Style::default().fg(Color::DarkGray)),
                Span::styled("q", Style::default().fg(Color::White)),
                Span::styled(" quit", Style::default().fg(Color::DarkGray)),
            ]),
            None => Line::from(vec![Span::styled(
                " / filter · ^f search · t time · ? help · q quit ",
                Style::default().fg(Color::DarkGray),
            )]),
        }
    };
    frame.render_widget(Paragraph::new(content), area);
}

fn draw_search(frame: &mut ratatui::Frame<'_>, _app: &App, state: &SearchState) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let scope_label = if state.global { "global" } else { "this repo" };
    let header = Line::from(vec![
        Span::styled(" search", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("   scope: {scope_label}   (tab to toggle · esc to close)"),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), layout[0]);

    let prompt = Line::from(vec![
        Span::styled(
            " > ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(state.query.clone()),
        Span::styled("_", Style::default().fg(Color::Yellow)),
    ]);
    frame.render_widget(Paragraph::new(prompt), layout[1]);

    if state.running {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  searching…",
                Style::default().fg(Color::DarkGray),
            )),
            layout[2],
        );
    } else if state.results.is_empty() {
        let msg = if state.query.is_empty() {
            "  type to search; enter to run"
        } else {
            "  no results"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(Color::DarkGray))),
            layout[2],
        );
    } else {
        let items: Vec<ListItem> = state
            .results
            .iter()
            .map(|h| {
                let pname = &h.project_name;
                let when = h.timestamp.map(relative_time).unwrap_or_else(|| "-".into());
                let kind = if h.session_id.is_some() {
                    "session"
                } else {
                    "anchor "
                };
                let snippet = truncate(&h.snippet, 60);
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {kind}"), Style::default().fg(Color::Magenta)),
                    Span::styled(format!("  {pname:<14}"), Style::default().fg(Color::Cyan)),
                    Span::styled(format!("  {when:<5}"), Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("  {snippet}")),
                ]))
            })
            .collect();
        let mut ls = state.list;
        frame.render_stateful_widget(
            List::new(items)
                .highlight_style(
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▸ "),
            layout[2],
            &mut ls,
        );
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " enter run/open · tab scope · ↑↓ nav · esc back ",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, layout[3]);
}

fn draw_picker_overlay(frame: &mut ratatui::Frame<'_>, p: &PickerState) {
    let area = frame.area();
    let height = (p.len() as u16 + 4).clamp(6, 20);
    let rect = centered(area, 70, height);
    frame.render_widget(Clear, rect);

    let items: Vec<ListItem> = (0..p.len())
        .map(|i| ListItem::new(Line::from(Span::raw(p.row_label(i)))))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", p.title))
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut ls = p.list;
    frame.render_stateful_widget(list, rect, &mut ls);
}

fn draw_help_overlay(frame: &mut ratatui::Frame<'_>) {
    let area = frame.area();
    let rect = centered(area, 62, 26);

    let lines = vec![
        Line::from(Span::styled(
            " anchor TUI · keybindings",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(" FEED"),
        Line::from("   ↑/k ↓/j     move selection"),
        Line::from("   g / G       top / bottom"),
        Line::from("   enter, s    open memory (session picker if >1)"),
        Line::from("   c           continue from selected working memory"),
        Line::from("   d           git show --color (diff)"),
        Line::from("   b           anchor blame (file picker if >1)"),
        Line::from("   /           live filter"),
        Line::from("   ctrl-f      full search (project/global)"),
        Line::from("   t           cycle time window (all / 24h / 7d / 30d)"),
        Line::from("   e           toggle tracking for this repo"),
        Line::from("   r           reload from db"),
        Line::from(""),
        Line::from(" MEMORY"),
        Line::from("   ↑↓ pgup/pgdn  scroll · g/G top/bot"),
        Line::from("   [ / ]         prev / next session of this anchor"),
        Line::from("   /             find in memory · n/N next/prev match"),
        Line::from("   esc/backspace back · q quit"),
        Line::from(""),
        Line::from(" SEARCH"),
        Line::from("   tab         toggle project ↔ global"),
        Line::from("   enter       run · (on a hit) open memory"),
        Line::from(""),
        Line::from(Span::styled(
            " press any key to dismiss",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    frame.render_widget(Clear, rect);
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" help ")
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(para, rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(subject: &str, intent: Option<&str>, tool: Option<&str>) -> AnchorRow {
        AnchorRow {
            kind: MemoryKind::Anchor,
            sha: "abcdef123456".into(),
            timestamp: 0,
            subject: subject.into(),
            intent: intent.map(str::to_string),
            tool: tool.map(str::to_string),
            tokens: None,
            session_count: 0,
            files: 0,
            tool_calls: 0,
            turn_index: None,
            session_id: None,
            parent_anchor: None,
        }
    }

    #[test]
    fn visible_anchors_match_subject_intent_sha_and_tool() {
        let rows = vec![
            anchor(
                "fix auth middleware",
                Some("refresh token flow"),
                Some("claude"),
            ),
            anchor("update docs", None, Some("cursor")),
        ];

        assert_eq!(visible_anchor_count(&rows, "auth"), 1);
        assert_eq!(visible_anchor_count(&rows, "refresh"), 1);
        assert_eq!(visible_anchor_count(&rows, "abcdef"), 2);
        assert_eq!(visible_anchor_count(&rows, "cursor"), 1);
        assert_eq!(visible_anchor_count(&rows, "missing"), 0);
    }

}
