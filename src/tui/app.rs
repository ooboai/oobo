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
//! (git show, oobo blame), and restore on return.

use std::io;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::config::Config;
use crate::db::Db;

// ── Public entry ──────────────────────────────────────────────────────

pub fn run(cfg: &Config) -> Result<i32, String> {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        return Err("not a git repository".to_string());
    };

    let mut app = App::load(cfg.clone(), root)?;
    if !app.enabled {
        println!("oobo disabled for this project. run: oobo enable");
        return Ok(0);
    }

    let mut terminal = super::init().map_err(|e| format!("tui init: {e}"))?;
    let result = event_loop(&mut terminal, &mut app);
    super::restore();
    result
}

/// Project-picker TUI for bare `oobo` run outside of a git repo.
///
/// Shows every tracked project with stats; pressing Enter drills into that
/// project's feed (reusing the same App/event loop). Quitting the feed
/// returns to the picker. `q`/`Esc` from the picker exits.
pub fn run_projects(cfg: &Config) -> Result<i32, String> {
    let mut terminal = super::init().map_err(|e| format!("tui init: {e}"))?;
    let result = project_picker_loop(cfg, &mut terminal);
    super::restore();
    result
}

// ── Project picker ────────────────────────────────────────────────────

#[derive(Clone)]
struct ProjectPickerRow {
    id: String,
    name: String,
    path: String,
    remote: Option<String>,
    enabled: bool,
    last_activity: i64,
    anchors: i64,
    tokens: i64,
    ai_pct: i64,
    /// Last tool that worked in the project (claude, cursor, codex, …).
    last_agent: Option<String>,
}

fn project_picker_loop(
    cfg: &Config,
    terminal: &mut ratatui::DefaultTerminal,
) -> Result<i32, String> {
    let mut rows = load_project_rows();
    if rows.is_empty() {
        // Nothing usable to show — bail so the caller's fallback can render.
        return Err("no projects".into());
    }

    let mut state = ListState::default();
    state.select(Some(0));
    let mut filter = String::new();
    let mut filter_open = false;

    loop {
        terminal
            .draw(|frame| draw_project_picker(frame, &rows, &filter, filter_open, &mut state))
            .map_err(|e| format!("tui draw: {e}"))?;

        let Some(key) = super::next_key(Duration::from_millis(200))
            .map_err(|e: io::Error| format!("key read: {e}"))?
        else {
            continue;
        };

        if filter_open {
            match key.code {
                KeyCode::Esc => {
                    filter.clear();
                    filter_open = false;
                }
                KeyCode::Enter => {
                    filter_open = false;
                }
                KeyCode::Backspace => {
                    filter.pop();
                }
                KeyCode::Char(c) => filter.push(c),
                _ => {}
            }
            state.select(Some(0));
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(0),
            KeyCode::Char('/') => {
                filter_open = true;
                filter.clear();
            }
            KeyCode::Char('r') => {
                rows = load_project_rows();
                state.select(Some(0));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let visible = visible_project_count(&rows, &filter);
                if visible > 0 {
                    let i = state.selected().unwrap_or(0);
                    state.select(Some((i + 1).min(visible - 1)));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = state.selected().unwrap_or(0);
                state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Home => {
                state.select(Some(0));
            }
            KeyCode::End => {
                let visible = visible_project_count(&rows, &filter);
                if visible > 0 {
                    state.select(Some(visible - 1));
                }
            }
            KeyCode::Enter => {
                let selected = state.selected().unwrap_or(0);
                let picked_path: Option<String> = visible_projects(&rows, &filter)
                    .nth(selected)
                    .map(|r| r.path.clone());
                if let Some(path) = picked_path {
                    // Suspend the picker, open the feed for this project,
                    // then re-init and resume.
                    super::restore();
                    let res = open_feed_for_project(cfg, &path);
                    *terminal = super::init().map_err(|e| format!("tui init: {e}"))?;
                    if let Err(e) = res {
                        eprintln!("oobo: could not open project: {e}");
                    }
                    // Refresh stats since anchors may have changed.
                    rows = load_project_rows();
                }
            }
            _ => {}
        }
    }
}

fn load_project_rows() -> Vec<ProjectPickerRow> {
    let Ok(db) = Db::open() else {
        return Vec::new();
    };
    let projects = db.list_projects().unwrap_or_default();
    let mut rows: Vec<ProjectPickerRow> = Vec::with_capacity(projects.len());
    for p in &projects {
        if is_stale_project_path(&p.path) {
            continue;
        }
        let settings = db.get_project_settings(&p.id).unwrap_or_default();
        let stats = db.anchor_stats_for_project(&p.id).unwrap_or_default();

        // Tokens across ALL sessions in the project (not just anchored ones).
        let session_tokens = project_session_tokens(&db, &p.id);
        let tokens = stats.tokens.max(session_tokens);

        // Last activity across multiple sources.
        let last_activity = project_last_activity(&db, &p.id, stats.last_activity);

        // Last agent to work in the project.
        let last_agent = project_last_agent(&db, &p.id);

        rows.push(ProjectPickerRow {
            id: p.id.clone(),
            name: p.name.clone(),
            path: p.path.clone(),
            remote: p.git_remote.clone(),
            enabled: !settings.ignored,
            last_activity,
            anchors: stats.anchors,
            tokens,
            ai_pct: stats.ai_pct,
            last_agent,
        });
    }
    // Sort: projects with activity first (most recent), zeros sink to bottom
    // alphabetically so it's stable.
    rows.sort_by(|a, b| {
        match (a.last_activity > 0, b.last_activity > 0) {
            (true, true) => b.last_activity.cmp(&a.last_activity),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    rows
}

fn project_session_tokens(db: &Db, project_id: &str) -> i64 {
    use rusqlite::params;
    db.conn
        .query_row(
            "SELECT COALESCE(SUM(
                COALESCE(st.input_tokens, 0)
                + COALESCE(st.output_tokens, 0)
                + COALESCE(st.cache_read_tokens, 0)
                + COALESCE(st.cache_creation_tokens, 0)
             ), 0)
             FROM session_stats st
             JOIN sessions s ON s.id = st.session_id AND s.source = st.source
             WHERE s.project_id = ?1",
            params![project_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
}

fn project_last_activity(db: &Db, project_id: &str, seed: i64) -> i64 {
    use rusqlite::params;
    let mut best = seed;

    if let Ok(ts) = db.conn.query_row(
        "SELECT COALESCE(MAX(updated_at), 0) FROM sessions WHERE project_id = ?1",
        params![project_id],
        |r| r.get::<_, i64>(0),
    ) {
        if ts > best {
            best = ts;
        }
    }

    if let Ok(ts) = db.conn.query_row(
        "SELECT COALESCE(MAX(a.committed_at), 0)
         FROM anchors a
         JOIN ai_commits c ON c.commit_hash = a.commit_hash
         WHERE c.project_id = ?1",
        params![project_id],
        |r| r.get::<_, i64>(0),
    ) {
        if ts > best {
            best = ts;
        }
    }

    if let Ok(ts) = db.conn.query_row(
        "SELECT COALESCE(MAX(timestamp), 0) FROM events WHERE project_id = ?1",
        params![project_id],
        |r| r.get::<_, i64>(0),
    ) {
        if ts > best {
            best = ts;
        }
    }

    best
}

fn project_last_agent(db: &Db, project_id: &str) -> Option<String> {
    use rusqlite::params;
    db.conn
        .query_row(
            "SELECT source FROM sessions
             WHERE project_id = ?1
             ORDER BY COALESCE(updated_at, created_at, 0) DESC
             LIMIT 1",
            params![project_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
}

fn is_stale_project_path(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    let tmp_prefixes = [
        "/tmp/",
        "/var/folders/",
        "/private/tmp/",
        "/private/var/folders/",
    ];
    if tmp_prefixes.iter().any(|p| path.starts_with(p)) {
        return true;
    }
    !std::path::Path::new(path).exists()
}

fn visible_projects<'a>(
    rows: &'a [ProjectPickerRow],
    filter: &'a str,
) -> impl Iterator<Item = &'a ProjectPickerRow> {
    let needle = filter.to_lowercase();
    rows.iter().filter(move |r| {
        if needle.is_empty() {
            return true;
        }
        r.name.to_lowercase().contains(&needle)
            || r.path.to_lowercase().contains(&needle)
            || r.remote
                .as_deref()
                .map(|s| s.to_lowercase().contains(&needle))
                .unwrap_or(false)
    })
}

fn visible_project_count(rows: &[ProjectPickerRow], filter: &str) -> usize {
    visible_projects(rows, filter).count()
}

fn open_feed_for_project(cfg: &Config, root: &str) -> Result<(), String> {
    let mut app = App::load(cfg.clone(), root.to_string())?;
    if !app.enabled {
        // Still allow read-only browsing of disabled projects.
        app.enabled = false;
    }
    let mut t = super::init().map_err(|e| format!("tui init: {e}"))?;
    let res = event_loop(&mut t, &mut app);
    super::restore();
    res.map(|_| ())
}

fn draw_project_picker(
    frame: &mut ratatui::Frame<'_>,
    rows: &[ProjectPickerRow],
    filter: &str,
    filter_open: bool,
    state: &mut ListState,
) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    // Header
    let total = rows.len();
    let filtered = visible_project_count(rows, filter);
    let count_str = if filter.is_empty() {
        format!("{total} projects")
    } else {
        format!("{filtered}/{total} projects")
    };
    let totals = project_totals(rows);
    let header = Line::from(vec![
        Span::styled(
            " oobo · projects",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "    {}  ·  {} anchors  ·  {} tok  ·  {}% AI ",
                count_str,
                totals.anchors,
                super::format_tokens(totals.tokens),
                totals.ai_pct
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), layout[0]);

    // List
    // Columns:
    //   ● | name (flex)  path (flex, dim)  agent  when  anchors  tokens  AI%  on|off
    let inner_w = layout[1].width.saturating_sub(2) as usize;
    // Fixed trailing metadata width — each column gets right-aligned space.
    // agent(8) + when(6) + anchors(6+1) + tokens(7) + ai%(5) + enabled(4) + gaps(~10)
    let meta_w = 50usize;
    let name_w = 22usize;
    let path_w = inner_w.saturating_sub(meta_w + name_w + 3).max(10);

    // Text color shown inside highlighted row so it stays readable on blue bg.
    let dim_normal = Style::default().fg(Color::DarkGray);

    let items: Vec<ListItem> = visible_projects(rows, filter)
        .map(|r| {
            let when = relative_time(r.last_activity);
            let tokens = super::format_tokens(r.tokens);
            let enabled_label = if r.enabled { "on" } else { "off" };
            let dot_style = if r.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let dot = if r.enabled { "●" } else { "○" };

            let name = pad_or_truncate(&r.name, name_w);
            let path_display = display_path(&r.path);
            let path = pad_or_truncate(&path_display, path_w);
            let agent_raw = r
                .last_agent
                .as_deref()
                .map(short_agent_label)
                .unwrap_or("-");
            let agent = pad_or_truncate(agent_raw, 8);

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {dot} "), dot_style),
                Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {path}"), dim_normal),
                Span::styled(format!("  {agent}"), Style::default().fg(Color::Cyan)),
                Span::styled(format!("  {when:>5}"), dim_normal),
                Span::styled(format!("  {:>4}a", r.anchors), dim_normal),
                Span::styled(format!("  {tokens:>6}"), dim_normal),
                Span::styled(format!("  {:>3}% AI", r.ai_pct), dim_normal),
                Span::styled(format!("  {enabled_label}"), dim_normal),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(
            // Blue background with white foreground reads well in both
            // light and dark terminals; bold makes the selection pop.
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, layout[1], state);

    // Footer
    let footer = if filter_open {
        Line::from(vec![
            Span::styled(
                " filter: ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(filter.to_string()),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::styled(
                "   (enter to apply · esc to clear)",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(Span::styled(
            " ↑↓ nav · enter open · / filter · r reload · q quit ",
            Style::default().fg(Color::DarkGray),
        ))
    };
    frame.render_widget(Paragraph::new(footer), layout[2]);
}

fn display_path(path: &str) -> String {
    if let Some(home) = std::env::var("HOME").ok() {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

fn short_agent_label(source: &str) -> &str {
    let s = source.to_lowercase();
    if s.contains("claude") {
        "claude"
    } else if s.contains("cursor") {
        "cursor"
    } else if s.contains("codex") {
        "codex"
    } else if s.contains("copilot") {
        "copilot"
    } else if s.contains("gemini") {
        "gemini"
    } else if s.contains("aider") {
        "aider"
    } else {
        // Best-effort: show first 8 chars of whatever source label is.
        source
    }
}

struct ProjectTotals {
    anchors: i64,
    tokens: i64,
    ai_pct: i64,
}

fn project_totals(rows: &[ProjectPickerRow]) -> ProjectTotals {
    let anchors: i64 = rows.iter().map(|r| r.anchors).sum();
    let tokens: i64 = rows.iter().map(|r| r.tokens).sum();
    let weighted: i64 = rows.iter().map(|r| r.anchors * r.ai_pct).sum();
    let ai_pct = if anchors == 0 { 0 } else { weighted / anchors };
    ProjectTotals {
        anchors,
        tokens,
        ai_pct,
    }
}

// ── Data ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AnchorRow {
    pub sha: String,
    pub timestamp: i64,
    pub subject: String,
    pub intent: Option<String>,
    pub tool: Option<String>,
    pub tokens: Option<i64>,
    pub session_count: usize,
}

#[derive(Clone)]
pub struct SessionLink {
    pub session_id: String,
    pub source: String,
    pub model: Option<String>,
    pub tokens: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeWindow {
    All,
    Day,
    Week,
    Month,
}

impl TimeWindow {
    fn label(self) -> &'static str {
        match self {
            TimeWindow::All => "all",
            TimeWindow::Day => "24h",
            TimeWindow::Week => "7d",
            TimeWindow::Month => "30d",
        }
    }
    fn cutoff(self) -> Option<i64> {
        let now = chrono::Utc::now().timestamp();
        match self {
            TimeWindow::All => None,
            TimeWindow::Day => Some(now - 86_400),
            TimeWindow::Week => Some(now - 7 * 86_400),
            TimeWindow::Month => Some(now - 30 * 86_400),
        }
    }
    fn cycle(self) -> Self {
        match self {
            TimeWindow::All => TimeWindow::Day,
            TimeWindow::Day => TimeWindow::Week,
            TimeWindow::Week => TimeWindow::Month,
            TimeWindow::Month => TimeWindow::All,
        }
    }
}

// ── App state ─────────────────────────────────────────────────────────

pub struct App {
    cfg: Config,
    root: String,
    project_id: String,
    project_name: String,
    enabled: bool,
    anchors: Vec<AnchorRow>,
    filter: String,
    time_window: TimeWindow,
    stack: Vec<View>,
    flash: Option<String>,
}

enum View {
    Feed(FeedState),
    Transcript(TranscriptState),
    Search(SearchState),
    Picker(PickerState),
    Help,
}

struct FeedState {
    list: ListState,
    filter_input_open: bool,
}

struct TranscriptState {
    sessions: Vec<SessionLink>,
    idx: usize,
    project_path: String,
    lines: Vec<Line<'static>>,
    scroll: u16,
    // in-transcript filter
    filter: String,
    filter_open: bool,
    match_lines: Vec<usize>, // line indices that contain filter match
    match_cursor: usize,     // current position in match_lines
}

struct SearchState {
    query: String,
    global: bool,
    results: Vec<crate::commands::search::Hit>,
    list: ListState,
    running: bool,
}

/// Generic modal picker used for session and file selection.
struct PickerState {
    title: String,
    list: ListState,
    kind: PickerKind,
}

enum PickerKind {
    Session {
        sessions: Vec<SessionLink>,
        project_path: String,
    },
    BlameFile {
        files: Vec<String>,
        sha: String,
        root: String,
    },
}

impl PickerState {
    fn len(&self) -> usize {
        match &self.kind {
            PickerKind::Session { sessions, .. } => sessions.len(),
            PickerKind::BlameFile { files, .. } => files.len(),
        }
    }
    fn row_label(&self, i: usize) -> String {
        match &self.kind {
            PickerKind::Session { sessions, .. } => sessions
                .get(i)
                .map(|s| {
                    let sid: String = s.session_id.chars().take(10).collect();
                    let model = s.model.clone().unwrap_or_else(|| "-".into());
                    let tok = if s.tokens > 0 {
                        super::format_tokens(s.tokens)
                    } else {
                        "-".into()
                    };
                    format!(" {sid:<10}  {:<10}  {model:<20}  {tok}", s.source)
                })
                .unwrap_or_default(),
            PickerKind::BlameFile { files, .. } => {
                files.get(i).cloned().unwrap_or_default()
            }
        }
    }
}

// ── Loading ───────────────────────────────────────────────────────────

impl App {
    fn load(cfg: Config, root: String) -> Result<Self, String> {
        let project_id = crate::project::id_for_root(&root);
        let project_name = std::path::Path::new(&root)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let db = Db::open()?;
        let settings = db.get_project_settings(&project_id).unwrap_or_default();
        let enabled = !settings.ignored;
        let anchors = load_anchors(&cfg, &db, 500, TimeWindow::All)?;

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
            enabled,
            anchors,
            filter: String::new(),
            time_window: TimeWindow::All,
            stack: vec![View::Feed(feed)],
            flash: None,
        })
    }

    fn reload_anchors(&mut self) {
        if let Ok(db) = Db::open() {
            if let Ok(rows) = load_anchors(&self.cfg, &db, 500, self.time_window) {
                self.anchors = rows;
            }
            let settings = db.get_project_settings(&self.project_id).unwrap_or_default();
            self.enabled = !settings.ignored;
        }
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
        visible_anchors(&self.anchors, &self.filter).nth(idx).cloned()
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

fn event_loop(
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
        KeyCode::Esc => return Ok(true),
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
        KeyCode::Char('d') => {
            if let Some(anchor) = app.selected_anchor() {
                suspend_and_run(terminal, || run_git_show(&app.root, &anchor.sha))?;
            }
        }
        KeyCode::Char('b') => open_blame_picker(app),
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::PageUp => move_selection(app, -10),
        KeyCode::PageDown => move_selection(app, 10),
        KeyCode::Char('g') => move_to(app, 0),
        KeyCode::Char('G') => move_to(app, usize::MAX),
        KeyCode::Enter | KeyCode::Char('s') => open_sessions_for_selected_anchor(app),
        _ => {}
    }
    Ok(false)
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
        let text = line.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
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
    let hits = crate::commands::search::collect_local(&app.cfg, &query, &opts)
        .unwrap_or_default();
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
        let sessions = load_sessions_for_anchor(&sha);
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

enum PickerAction {
    OpenSession {
        session: SessionLink,
        project_path: String,
        siblings: Vec<SessionLink>,
        idx: usize,
    },
    Blame {
        root: String,
        file: String,
        sha: String,
    },
    Noop,
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

/// Open transcript for the selected anchor. If it has a single session,
/// open it directly. If multiple, push a session picker.
fn open_sessions_for_selected_anchor(app: &mut App) {
    let Some(anchor) = app.selected_anchor() else {
        return;
    };
    let sessions = load_sessions_for_anchor(&anchor.sha);
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
    let Ok(db) = Db::open() else {
        app.flash("cannot open db");
        return;
    };
    let mut settings = db.get_project_settings(&app.project_id).unwrap_or_default();
    settings.ignored = !settings.ignored;
    if db
        .set_project_settings(&app.project_id, &settings)
        .is_err()
    {
        app.flash("cannot update settings");
        return;
    }
    app.enabled = !settings.ignored;
    app.flash(if app.enabled {
        "tracking enabled"
    } else {
        "tracking disabled"
    });
}

// ── External commands (suspend/restore TUI) ──────────────────────────

fn suspend_and_run<F>(
    terminal: &mut ratatui::DefaultTerminal,
    f: F,
) -> Result<(), String>
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
    let oobo = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("oobo"));
    let status = std::process::Command::new(oobo)
        .args(["blame", file, sha])
        .current_dir(root)
        .status()
        .map_err(|e| format!("spawn oobo blame: {e}"))?;
    if !status.success() {
        return Err(format!("oobo blame exited {status}"));
    }
    println!("\npress enter to return...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
    Ok(())
}

// ── DB loaders ────────────────────────────────────────────────────────

fn load_anchors(
    cfg: &Config,
    db: &Db,
    limit: usize,
    window: TimeWindow,
) -> Result<Vec<AnchorRow>, String> {
    // Walk git log as source of truth (authoritative subject + timestamp),
    // then enrich each commit with AI session data from the DB.
    let n = limit.max(1);
    let log = crate::git::proxy::run_git_capture(
        cfg,
        &[
            "log",
            &format!("-{}", n),
            "--format=%H|||%s|||%ct",
        ],
    )
    .unwrap_or_default();

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

        // Pull intent from anchors table when present; otherwise use commit subject.
        let (intent, _msg_db) = load_anchor_meta(db, &sha);
        let subject = intent
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if subject_git.is_empty() {
                    "(no subject)".to_string()
                } else {
                    subject_git.clone()
                }
            });

        let summary = summarize_sessions(db, &sha);
        rows.push(AnchorRow {
            sha,
            timestamp: ts,
            subject,
            intent,
            tool: summary.tool,
            tokens: summary.tokens,
            session_count: summary.count,
        });
        if rows.len() >= limit {
            break;
        }
    }
    Ok(rows)
}

fn load_anchor_meta(db: &Db, commit_hash: &str) -> (Option<String>, Option<String>) {
    use rusqlite::params;
    db.conn
        .query_row(
            "SELECT intent, message FROM anchors WHERE commit_hash = ?1",
            params![commit_hash],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .unwrap_or((None, None))
}

struct AnchorSummary {
    tool: Option<String>,
    tokens: Option<i64>,
    count: usize,
}

fn summarize_sessions(db: &Db, commit_hash: &str) -> AnchorSummary {
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
    AnchorSummary {
        tool,
        tokens: if tokens == 0 { None } else { Some(tokens) },
        count,
    }
}

fn load_sessions_for_anchor(commit_hash: &str) -> Vec<SessionLink> {
    use rusqlite::params;
    let Ok(db) = Db::open() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let sql = "SELECT session_id, agent, model,
                      COALESCE(input_tokens,0) + COALESCE(output_tokens,0)
                      + COALESCE(cache_read_tokens,0) + COALESCE(cache_creation_tokens,0)
               FROM anchor_sessions WHERE commit_hash = ?1";
    if let Ok(mut stmt) = db.conn.prepare(sql) {
        if let Ok(rows) = stmt.query_map(params![commit_hash], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        }) {
            for r in rows.flatten() {
                out.push(SessionLink {
                    session_id: r.0,
                    source: r.1,
                    model: r.2,
                    tokens: r.3,
                });
            }
        }
    }
    out
}

fn lookup_project_path(project_id: &str) -> Option<String> {
    use rusqlite::params;
    let db = Db::open().ok()?;
    db.conn
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
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

fn load_transcript_lines(project_root: &str, s: &SessionLink) -> Vec<Line<'static>> {
    let messages =
        crate::session::parse_messages_for_session(project_root, &s.session_id, &s.source);
    if messages.is_empty() {
        return vec![
            Line::from(Span::styled(
                "(transcript not available for this session)",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("session {} · {}", &s.session_id, s.source),
                Style::default().fg(Color::DarkGray),
            )),
        ];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for m in messages {
        let role = role_label(&m.role);
        let role_style = role_style(&m.role);
        out.push(Line::from(vec![
            Span::styled(
                format!("[{role}] "),
                role_style.add_modifier(Modifier::BOLD),
            ),
            Span::raw(timestamp_short(m.timestamp_ms)),
        ]));
        for l in m.text.lines() {
            out.push(Line::from(Span::raw(l.to_string())));
        }
        out.push(Line::from(""));
    }
    out
}

fn role_label(role: &str) -> &'static str {
    match role {
        "user" => "you",
        "assistant" => "ai",
        "system" => "sys",
        "tool" => "tool",
        _ => "?",
    }
}

fn role_style(role: &str) -> Style {
    match role {
        "user" => Style::default().fg(Color::Cyan),
        "assistant" => Style::default().fg(Color::Green),
        "system" => Style::default().fg(Color::DarkGray),
        "tool" => Style::default().fg(Color::Magenta),
        _ => Style::default(),
    }
}

fn timestamp_short(ts_ms: Option<i64>) -> String {
    match ts_ms {
        Some(ms) => chrono::DateTime::from_timestamp(ms / 1000, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

// ── Rendering ─────────────────────────────────────────────────────────

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    // Always draw the root view (Feed) as the base. Then, if another view is
    // on top, overlay it. This keeps the feed visible behind overlays like
    // Help and Picker.
    if let Some(first) = app.stack.first() {
        match first {
            View::Feed(feed) => draw_feed(frame, app, feed),
            View::Transcript(ts) => draw_transcript(frame, app, ts),
            View::Search(s) => draw_search(frame, app, s),
            View::Picker(_) | View::Help => {}
        }
    }

    // Draw additional views on top of the root (if any).
    if app.stack.len() > 1 {
        if let Some(top) = app.stack.last() {
            match top {
                View::Feed(_) => {} // already drawn as root
                View::Transcript(ts) => draw_transcript(frame, app, ts),
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
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, layout[0]);

    // Empty state
    if app.anchors.is_empty() {
        draw_empty_state(frame, app, layout[1]);
        draw_footer(frame, app, feed, layout[2]);
        return;
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(layout[1]);

    draw_anchor_list(frame, app, feed, body[0]);
    draw_anchor_detail(frame, app, body[1]);
    draw_footer(frame, app, feed, layout[2]);
}

fn draw_empty_state(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let hints: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            format!("  no anchors yet for \"{}\"", app.project_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  an anchor is created each time you commit with oobo active.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  · make a commit:   oobo commit -m \"your message\"",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "  · (re)install git alias:  oobo alias install",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "  · index existing sessions: oobo setup",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  press ? for keybindings · q to quit",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(hints).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_header(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let total_anchors = app.anchors.len();
    let filtered = visible_anchor_count(&app.anchors, &app.filter);
    let count_str = if app.filter.is_empty() {
        format!("{total_anchors} anchors")
    } else {
        format!("{filtered}/{total_anchors} anchors")
    };
    let enabled = if app.enabled { "on" } else { "off" };
    let line = Line::from(vec![
        Span::styled(
            format!(" oobo · {}", app.project_name),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "    {} · window: {} · tracking: {} ",
                count_str,
                app.time_window.label(),
                enabled
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Left), area);
}

fn draw_anchor_list(
    frame: &mut ratatui::Frame<'_>,
    app: &App,
    feed: &FeedState,
    area: Rect,
) {
    // Reserve columns for the trailing metadata (tokens/sessions), dot and time.
    // Layout: " ● " (3) + "when " (6) + subject (flex) + "  tokens sessions" (~14)
    let total_w = area.width.saturating_sub(2) as usize;
    let meta_w = 14usize;
    let prefix_w = 3 + 6;
    let subject_w = total_w.saturating_sub(meta_w + prefix_w).max(12);

    let items: Vec<ListItem> = visible_anchors(&app.anchors, &app.filter)
        .map(|r| {
            let when = relative_time(r.timestamp);
            let has_ai = r.session_count > 0;
            let dot_style = if has_ai {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let dot = if has_ai { "●" } else { "○" };

            let tok_str = r
                .tokens
                .filter(|t| *t > 0)
                .map(super::format_tokens)
                .unwrap_or_else(|| "-".into());
            let sess_str = if r.session_count > 0 {
                format!("{}s", r.session_count)
            } else {
                "-".into()
            };
            let meta = format!("{tok_str:>6}  {sess_str:>4}");

            let subject_raw = if r.subject.is_empty() {
                "(no subject)".to_string()
            } else {
                r.subject.clone()
            };
            let subject = pad_or_truncate(&subject_raw, subject_w);

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {dot} "), dot_style),
                Span::styled(format!("{when:<5} "), Style::default().fg(Color::DarkGray)),
                Span::raw(subject),
                Span::styled(
                    format!("  {meta}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::RIGHT))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = feed.list.clone();
    frame.render_stateful_widget(list, area, &mut state);
}

fn pad_or_truncate(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count > width {
        let keep = width.saturating_sub(1);
        let mut out: String = s.chars().take(keep).collect();
        out.push('…');
        out
    } else {
        let mut out = s.to_string();
        for _ in 0..(width - count) {
            out.push(' ');
        }
        out
    }
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

    let sessions = load_sessions_for_anchor(&anchor.sha);
    let files = touched_files_for(&app.root, &anchor.sha);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}", short_sha(&anchor.sha)),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", relative_time(anchor.timestamp)),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
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
        Span::styled("  tool    ", Style::default().fg(Color::DarkGray)),
        Span::raw(tool.to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  tokens  ", Style::default().fg(Color::DarkGray)),
        Span::raw(tokens),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  sessions", Style::default().fg(Color::DarkGray)),
        Span::raw(format!(" {}", anchor.session_count)),
    ]));

    if !sessions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  SESSIONS",
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
            format!("  FILES ({})", files.len()),
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

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

fn draw_footer(
    frame: &mut ratatui::Frame<'_>,
    app: &App,
    feed: &FeedState,
    area: Rect,
) {
    let content: Line<'static> = if feed.filter_input_open {
        Line::from(vec![
            Span::styled(
                " filter: ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
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
    } else {
        Line::from(vec![Span::styled(
            " ↑↓ nav · enter sessions · d diff · b blame · / filter · ^f search · t time · e toggle · ? help · q quit ",
            Style::default().fg(Color::DarkGray),
        )])
    };
    frame.render_widget(Paragraph::new(content), area);
}

fn draw_transcript(frame: &mut ratatui::Frame<'_>, _app: &App, ts: &TranscriptState) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let session = &ts.sessions[ts.idx];
    let short_sid: String = session.session_id.chars().take(8).collect();
    let model = session.model.clone().unwrap_or_default();
    let position = if ts.sessions.len() > 1 {
        format!("  [{}/{}]", ts.idx + 1, ts.sessions.len())
    } else {
        String::new()
    };
    let matches = if ts.match_lines.is_empty() {
        String::new()
    } else {
        format!(
            "  {}/{} matches",
            ts.match_cursor + 1,
            ts.match_lines.len()
        )
    };
    let header = Line::from(vec![
        Span::styled(
            " transcript",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {}  {}  {}{}{}",
                short_sid, session.source, model, position, matches
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), layout[0]);

    // Highlight matching lines by colouring them.
    let rendered: Vec<Line<'static>> = ts
        .lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if ts.match_lines.contains(&i) {
                let mut new = line.clone();
                for span in new.spans.iter_mut() {
                    span.style = span.style.bg(Color::Yellow).fg(Color::Black);
                }
                new
            } else {
                line.clone()
            }
        })
        .collect();

    let max_scroll = rendered
        .len()
        .saturating_sub(layout[1].height as usize) as u16;
    let scroll = ts.scroll.min(max_scroll);
    let body = Paragraph::new(rendered)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(body, layout[1]);

    let footer_content: Line<'static> = if ts.filter_open {
        Line::from(vec![
            Span::styled(
                " /",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(ts.filter.clone()),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::styled(
                "   (enter to jump · esc to clear)",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(Span::styled(
            " ↑↓ scroll · pgup/pgdn page · g/G top/bot · [ / ] prev/next session · / find · n/N · esc back · q quit ",
            Style::default().fg(Color::DarkGray),
        ))
    };
    frame.render_widget(Paragraph::new(footer_content), layout[2]);
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
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
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
                    Span::styled(
                        format!(" {kind}"),
                        Style::default().fg(Color::Magenta),
                    ),
                    Span::styled(
                        format!("  {pname:<14}"),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("  {when:<5}"),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(format!("  {snippet}")),
                ]))
            })
            .collect();
        let mut ls = state.list.clone();
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
    let mut ls = p.list.clone();
    frame.render_stateful_widget(list, rect, &mut ls);
}

fn draw_help_overlay(frame: &mut ratatui::Frame<'_>) {
    let area = frame.area();
    let rect = centered(area, 62, 26);

    let lines = vec![
        Line::from(Span::styled(
            " oobo TUI · keybindings",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(" FEED"),
        Line::from("   ↑/k ↓/j     move selection"),
        Line::from("   g / G       top / bottom"),
        Line::from("   enter, s    open transcript (session picker if >1)"),
        Line::from("   d           git show --color (diff)"),
        Line::from("   b           oobo blame (file picker if >1)"),
        Line::from("   /           live filter"),
        Line::from("   ctrl-f      full search (project/global)"),
        Line::from("   t           cycle time window (all / 24h / 7d / 30d)"),
        Line::from("   e           toggle tracking for this repo"),
        Line::from("   r           reload from db"),
        Line::from(""),
        Line::from(" TRANSCRIPT"),
        Line::from("   ↑↓ pgup/pgdn  scroll · g/G top/bot"),
        Line::from("   [ / ]         prev / next session of this anchor"),
        Line::from("   /             find in transcript · n/N next/prev match"),
        Line::from("   esc/backspace back · q quit"),
        Line::from(""),
        Line::from(" SEARCH"),
        Line::from("   tab         toggle project ↔ global"),
        Line::from("   enter       run · (on a hit) open transcript"),
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

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}

// ── Utilities ─────────────────────────────────────────────────────────

fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        format!("{s:<max$}")
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
