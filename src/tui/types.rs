use ratatui::text::Line;
use ratatui::widgets::ListState;

use crate::feed::{FeedRow, RowKind};

#[derive(Clone)]
pub struct AnchorRow {
    pub kind: MemoryKind,
    pub sha: String,
    pub timestamp: i64,
    pub subject: String,
    pub intent: Option<String>,
    pub tool: Option<String>,
    pub tokens: Option<i64>,
    pub session_count: usize,
    pub files: usize,
    pub tool_calls: usize,
    pub session_id: Option<String>,
    pub parent_anchor: Option<String>,
    pub worktree_hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryKind {
    Anchor,
    ShadowAnchor,
}

impl From<FeedRow> for AnchorRow {
    fn from(r: FeedRow) -> Self {
        let subject = r
            .intent
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if r.subject.is_empty() {
                    "(no subject)".to_string()
                } else {
                    r.subject.clone()
                }
            });
        AnchorRow {
            kind: match r.kind {
                RowKind::Anchor => MemoryKind::Anchor,
                RowKind::Shadow => MemoryKind::ShadowAnchor,
            },
            sha: r.id,
            timestamp: r.timestamp,
            subject,
            intent: r.intent,
            tool: r.tool,
            tokens: if r.tokens > 0 { Some(r.tokens) } else { None },
            session_count: r.session_count,
            files: r.files,
            tool_calls: r.tool_calls,
            session_id: r.session_id,
            parent_anchor: r.parent_anchor,
            worktree_hint: r.worktree_hint,
        }
    }
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
    pub(super) fn label(self) -> &'static str {
        match self {
            TimeWindow::All => "all",
            TimeWindow::Day => "24h",
            TimeWindow::Week => "7d",
            TimeWindow::Month => "30d",
        }
    }

    pub(super) fn cutoff(self) -> Option<i64> {
        let now = chrono::Utc::now().timestamp();
        match self {
            TimeWindow::All => None,
            TimeWindow::Day => Some(now - 86_400),
            TimeWindow::Week => Some(now - 7 * 86_400),
            TimeWindow::Month => Some(now - 30 * 86_400),
        }
    }

    pub(super) fn cycle(self) -> Self {
        match self {
            TimeWindow::All => TimeWindow::Day,
            TimeWindow::Day => TimeWindow::Week,
            TimeWindow::Week => TimeWindow::Month,
            TimeWindow::Month => TimeWindow::All,
        }
    }
}

pub(super) enum View {
    Feed(FeedState),
    Search(SearchState),
    CodeSearch {
        query: String,
        results: Vec<sonar_core::types::SearchResult>,
    },
    Transcript(TranscriptState),
    Diff(DiffState),
    Picker(PickerState),
    Help,
}

/// Scrollable commit diff rendered in-TUI (replaces git-show shell-out).
pub(super) struct DiffState {
    pub(super) sha: String,
    pub(super) subject: String,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) scroll: u16,
}

pub(super) struct FeedState {
    pub(super) list: ListState,
    pub(super) filter_input_open: bool,
}

pub(super) struct TranscriptState {
    pub(super) sessions: Vec<SessionLink>,
    pub(super) idx: usize,
    pub(super) project_path: String,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) scroll: u16,
    pub(super) filter: String,
    pub(super) filter_open: bool,
    pub(super) match_lines: Vec<usize>,
    pub(super) match_cursor: usize,
    /// Line indices where tool-call messages begin (for t/T navigation).
    pub(super) tool_call_lines: Vec<usize>,
    pub(super) tool_call_cursor: usize,
}

/// Generic modal picker used for session and file selection.
pub(super) struct PickerState {
    pub(super) title: String,
    pub(super) list: ListState,
    pub(super) kind: PickerKind,
}

pub(super) enum PickerKind {
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
    pub(super) fn len(&self) -> usize {
        match &self.kind {
            PickerKind::Session { sessions, .. } => sessions.len(),
            PickerKind::BlameFile { files, .. } => files.len(),
        }
    }

    pub(super) fn row_label(&self, i: usize) -> String {
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
            PickerKind::BlameFile { files, .. } => files.get(i).cloned().unwrap_or_default(),
        }
    }
}

pub(super) enum PickerAction {
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

// ── Remote search ────────────────────────────────────────────────────

#[derive(Clone)]
pub(super) struct SearchResultRow {
    pub anchor_sha: Option<String>,
    pub project_name: String,
    pub snippet: String,
    pub score: f64,
    /// `"fts"`, `"memory"`, or `"local"`.
    pub source: String,
    pub tool: Option<String>,
    pub tokens: Option<i64>,
    pub timestamp: Option<i64>,
    pub author: Option<String>,
}

pub(super) enum SearchStatus {
    Input,
    Loading,
    Done,
    Error(String),
    NoApiKey,
}

pub(super) struct SearchResponse {
    pub answer: Option<String>,
    pub results: Vec<SearchResultRow>,
}

pub(super) struct SearchState {
    pub input: String,
    pub query: String,
    pub answer: Option<String>,
    pub results: Vec<SearchResultRow>,
    pub list: ListState,
    pub status: SearchStatus,
    pub rx: Option<std::sync::mpsc::Receiver<Result<SearchResponse, String>>>,
}
