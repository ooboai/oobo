use ratatui::text::Line;
use ratatui::widgets::ListState;

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
    pub turn_index: Option<i64>,
    pub session_id: Option<String>,
    pub parent_anchor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryKind {
    Anchor,
    ShadowAnchor,
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
    Transcript(TranscriptState),
    Search(SearchState),
    Picker(PickerState),
    Help,
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
}

pub(super) struct SearchState {
    pub(super) query: String,
    pub(super) global: bool,
    pub(super) results: Vec<crate::commands::search::Hit>,
    pub(super) list: ListState,
    pub(super) running: bool,
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
