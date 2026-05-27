//! Unified TUI  --  the single entry point for `oobo`, `oobo recall`, `oobo search`,
//! and `oobo anchor show`.
//!
//! Architecture: a single [`App`] owns all state (anchors, filter, time
//! window, config) and a `Vec<View>` view-stack. The top of the stack is
//! the active view. `Esc` pops; `q` quits from root.
//!
//! Views:
//! - [`View::Feed`]        list + detail (with inline `/` filter)
//! - [`View::Transcript`]  scrollable session transcript
//! - [`View::Picker`]      modal list (session / file chooser)
//! - [`View::Help`]        keybindings overlay
//!
//! Some keys suspend the TUI and run a subprocess (`oobo blame`,
//! `oobo goto`), then restore on return. See `external.rs`.
//! Diffs (`d`) are rendered inline in a scrollable `View::Diff`.

use std::io;
use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::widgets::ListState;

use crate::config::Config;
use crate::error::{CliError, CmdResult};

use super::data::{current_branch, load_anchors, worktree_dirty};
use super::draw::{draw, draw_loading_skeleton, LoadingSkeleton};
use super::input::handle_key;
use super::types::{AnchorRow, FeedState, TimeWindow, View};

// ── Shared bootstrap ─────────────────────────────────────────────────

/// Spin up the TUI terminal, load data on a background thread with an
/// animated skeleton, and return the ready `(terminal, App)`.
/// Returns `Ok(None)` if the user quit during loading.
fn bootstrap(cfg: &Config) -> Result<Option<(ratatui::DefaultTerminal, App)>, CliError> {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        return Err(CliError::NotARepo);
    };

    let mut terminal = super::init().map_err(|e| CliError::User(format!("tui init: {e}")))?;

    let project_name = std::path::Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();
    let enabled = crate::project_config::is_enabled(&root);
    let branch = current_branch(&root);

    let cfg_clone = cfg.clone();
    let root_clone = root.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(App::load(cfg_clone, root_clone));
    });

    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut i = 0usize;
    let skeleton = LoadingSkeleton {
        project_name: &project_name,
        enabled,
        branch: branch.as_deref(),
    };

    let app = loop {
        let _ = terminal.draw(|f| draw_loading_skeleton(f, &skeleton, FRAMES[i]));

        match rx.try_recv() {
            Ok(result) => break result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                super::restore();
                return Err(CliError::User("loading thread panicked".into()));
            }
        }

        if crossterm::event::poll(Duration::from_millis(80)).unwrap_or(false) {
            if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    super::restore();
                    return Ok(None);
                }
            }
        }
        i = (i + 1) % FRAMES.len();
    };

    match app {
        Ok(app) => Ok(Some((terminal, app))),
        Err(e) => {
            super::restore();
            Err(e)
        }
    }
}

/// Run the event loop and clean up the terminal on exit.
fn run_tui(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> CmdResult {
    let result = event_loop(terminal, app);
    super::restore();
    result
}

// ── Public entry ──────────────────────────────────────────────────────

pub fn run(cfg: &Config) -> CmdResult {
    let Some((mut terminal, mut app)) = bootstrap(cfg)? else {
        return Ok(0);
    };
    run_tui(&mut terminal, &mut app)
}

// ── App state ─────────────────────────────────────────────────────────

pub(super) struct App {
    pub(super) cfg: Config,
    pub(super) root: String,
    pub(super) project_id: String,
    pub(super) project_name: String,
    pub(super) branch: Option<String>,
    pub(super) anchor_remote: String,
    pub(super) dirty: bool,
    pub(super) enabled: bool,
    pub(super) anchors: Vec<AnchorRow>,
    pub(super) filter: String,
    pub(super) time_window: TimeWindow,
    pub(super) stack: Vec<View>,
    pub(super) flash: Option<String>,
    pub(super) tick: usize,
}

pub(super) const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// ── Loading ───────────────────────────────────────────────────────────

impl App {
    #[tracing::instrument(skip_all)]
    pub(super) fn load(cfg: Config, root: String) -> Result<Self, CliError> {
        let project_id = crate::project::id_for_root(&root);
        let project_name = std::path::Path::new(&root)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let enabled = crate::project_config::is_enabled(&root);
        let anchors = load_anchors(&cfg, &root, 500, TimeWindow::All)?;
        tracing::debug!(anchor_count = anchors.len(), "TUI anchors loaded");
        let branch = current_branch(&root);
        let anchor_remote = crate::commands::sync::resolve(&cfg, Some(&root)).anchor_remote;
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
            tick: 0,
        })
    }

    pub(super) fn reload_anchors(&mut self) {
        match load_anchors(&self.cfg, &self.root, 500, self.time_window) {
            Ok(rows) => self.anchors = rows,
            Err(e) => {
                self.flash = Some(format!("reload failed: {e}"));
            }
        }
        self.enabled = crate::project_config::is_enabled(&self.root);
        self.branch = current_branch(&self.root);
        self.anchor_remote =
            crate::commands::sync::resolve(&self.cfg, Some(&self.root)).anchor_remote;
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

    pub(super) fn selected_anchor(&self) -> Option<AnchorRow> {
        let View::Feed(feed) = self.stack.first()? else {
            return None;
        };
        let idx = feed.list.selected()?;
        visible_anchors(&self.anchors, &self.filter)
            .nth(idx)
            .cloned()
    }

    pub(super) fn flash(&mut self, msg: impl Into<String>) {
        self.flash = Some(msg.into());
    }

    /// Launch a remote search in a background thread. Returns a SearchState
    /// ready to be pushed onto the view stack.
    pub(super) fn start_search(&self, query: String) -> super::types::SearchState {
        use super::types::{SearchState, SearchStatus};

        let resolved = crate::commands::sync::resolve(&self.cfg, Some(&self.root));
        if !resolved.has_api_key() {
            return SearchState {
                input: query.clone(),
                query,
                answer: None,
                results: Vec::new(),
                list: ratatui::widgets::ListState::default(),
                status: SearchStatus::NoApiKey,
                rx: None,
            };
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let api_key = resolved.api_key.clone();
        let api_url = resolved.api_url.clone();
        let q = query.clone();

        std::thread::spawn(move || {
            let result = run_blocking_search(&api_key, &api_url, &q);
            let _ = tx.send(result);
        });

        SearchState {
            input: query.clone(),
            query,
            answer: None,
            results: Vec::new(),
            list: ratatui::widgets::ListState::default(),
            status: SearchStatus::Loading,
            rx: Some(rx),
        }
    }
}

fn run_blocking_search(
    api_key: &str,
    api_url: &str,
    query: &str,
) -> Result<super::types::SearchResponse, String> {
    use super::types::{SearchResponse, SearchResultRow};

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let request = crate::remote::payload::SearchRequest {
        query: query.to_string(),
        since: None,
        project: Some(crate::remote::payload::SearchProjectScope {
            kind: "global".to_string(),
            value: None,
        }),
        tool: None,
        limit: 20,
    };

    let response = rt
        .block_on(crate::remote::search_anchors_with_timeout(
            &request,
            api_key,
            api_url,
            std::time::Duration::from_secs(15),
        ))
        .map_err(|e| format!("{e}"))?;

    let results = response
        .hits
        .into_iter()
        .map(|h| {
            let short_sha = h.anchor_sha.as_ref().map(|s| s.chars().take(7).collect());
            SearchResultRow {
                anchor_sha: short_sha,
                project_name: h.project.name.unwrap_or_else(|| "remote".to_string()),
                snippet: h.snippet.unwrap_or_default(),
                score: h.score.unwrap_or(0.0),
                source: h.source.unwrap_or_else(|| "fts".to_string()),
                tool: h.tool,
                tokens: h.tokens,
                timestamp: h.timestamp,
                author: h.author,
            }
        })
        .collect();

    Ok(SearchResponse {
        answer: response.answer,
        results,
    })
}

pub(super) fn visible_anchors<'a>(
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
                .is_some_and(|s| s.to_ascii_lowercase().contains(&f))
            || r.intent
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&f))
            || r.tool
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&f))
    })
}

pub(super) fn visible_anchor_count(rows: &[AnchorRow], filter: &str) -> usize {
    visible_anchors(rows, filter).count()
}

// ── Search entry point ────────────────────────────────────────────

/// Opens the TUI with the recall (memory/session search) view. If a query is
/// provided, fires the search immediately; otherwise opens the search input.
pub fn run_recall(cfg: &Config, query: &str) -> CmdResult {
    use super::types::{SearchState, SearchStatus, View};

    let Some((mut terminal, mut app)) = bootstrap(cfg)? else {
        return Ok(0);
    };

    if query.is_empty() {
        app.stack.push(View::Search(SearchState {
            input: String::new(),
            query: String::new(),
            answer: None,
            results: Vec::new(),
            list: ratatui::widgets::ListState::default(),
            status: SearchStatus::Input,
            rx: None,
        }));
    } else {
        let ss = app.start_search(query.to_string());
        app.stack.push(View::Search(ss));
    }

    run_tui(&mut terminal, &mut app)
}

/// Opens the TUI with code search results (powered by sonar).
pub fn run_code_search(
    cfg: &Config,
    query: &str,
    path: &str,
    top_k: usize,
    mode: &str,
    content: &str,
) -> CmdResult {
    use super::types::View;

    let Some((mut terminal, mut app)) = bootstrap(cfg)? else {
        return Ok(0);
    };

    let results = crate::sonar::search_codebase(query, path, top_k, mode, content, None)
        .unwrap_or_default();

    app.stack.push(View::CodeSearch {
        query: query.to_string(),
        results,
    });

    run_tui(&mut terminal, &mut app)
}

// ── Show entry point ─────────────────────────────────────────────

/// Same TUI as `oobo`, but pre-focused on one anchor's detail view.
pub fn run_show(cfg: &Config, commit_sha: &str) -> CmdResult {
    let Some((mut terminal, mut app)) = bootstrap(cfg)? else {
        return Ok(0);
    };

    if let Some(View::Feed(feed)) = app.stack.first_mut() {
        let target = commit_sha.to_lowercase();
        if let Some(pos) = app.anchors.iter().position(|a| {
            a.sha.to_lowercase().starts_with(&target) || a.sha.to_lowercase() == target
        }) {
            feed.list.select(Some(pos));
            super::input::open_selected_memory_public(&mut app);
        }
    }

    run_tui(&mut terminal, &mut app)
}

// ── Event loop ────────────────────────────────────────────────────────

pub(super) fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> CmdResult {
    loop {
        poll_search_results(app);
        app.tick = app.tick.wrapping_add(1);

        terminal
            .draw(|frame| draw(frame, app))
            .map_err(|e| CliError::User(format!("tui draw: {e}")))?;

        let Some(key) =
            super::next_key(Duration::from_millis(100)).map_err(|e: io::Error| CliError::Io {
                context: "key read".into(),
                source: e,
            })?
        else {
            continue;
        };

        app.flash = None;

        if handle_key(terminal, app, key)? {
            return Ok(0);
        }
    }
}

fn poll_search_results(app: &mut App) {
    use super::types::{SearchStatus, View};

    let Some(View::Search(ss)) = app.stack.last_mut() else {
        return;
    };
    let Some(rx) = &ss.rx else {
        return;
    };
    match rx.try_recv() {
        Ok(Ok(resp)) => {
            let count = resp.results.len();
            ss.answer = resp.answer;
            ss.results = resp.results;
            ss.status = SearchStatus::Done;
            ss.rx = None;
            if count > 0 {
                ss.list.select(Some(0));
            }
        }
        Ok(Err(e)) => {
            ss.status = SearchStatus::Error(e);
            ss.rx = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            ss.status = SearchStatus::Error("search thread panicked".to_string());
            ss.rx = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::MemoryKind;
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
            session_id: None,
            parent_anchor: None,
            worktree_hint: None,
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
