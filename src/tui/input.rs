use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;

use super::app::{visible_anchor_count, App};
use super::data::{load_sessions_for_anchor, touched_files_for};
use super::external::{run_oobo_blame, run_oobo_goto, suspend_and_run};
use super::format::short_sha;
use super::transcript::load_transcript_lines;
use super::types::{
    DiffState, FeedState, MemoryKind, PickerAction, PickerKind, PickerState, SearchState,
    SearchStatus, SessionLink, TranscriptState, View,
};

// ── Key dispatcher ────────────────────────────────────────────────────

pub(super) fn handle_key(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, String> {
    let in_filter = matches!(
        app.stack.last(),
        Some(View::Feed(FeedState {
            filter_input_open: true,
            ..
        }))
    );
    let in_search = matches!(app.stack.last(), Some(View::Search(_)));
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        if in_filter {
            if let Some(View::Feed(feed)) = app.stack.last_mut() {
                app.filter.clear();
                feed.filter_input_open = false;
                let total = visible_anchor_count(&app.anchors, &app.filter);
                if total > 0 {
                    feed.list.select(Some(0));
                } else {
                    feed.list.select(None);
                }
            }
            return Ok(false);
        }
        if in_search {
            app.stack.pop();
            return Ok(false);
        }
        return Ok(true);
    }

    if matches!(app.stack.last(), Some(View::Help)) {
        app.stack.pop();
        return Ok(false);
    }

    match app.stack.last_mut() {
        Some(View::Feed(_)) => handle_feed_key(terminal, app, key),
        Some(View::Search(_)) => Ok(handle_search_key(app, key)),
        Some(View::CodeSearch { .. }) => Ok(handle_code_search_key(app, key)),
        Some(View::Transcript(_)) => Ok(handle_transcript_key(app, key)),
        Some(View::Diff(_)) => Ok(handle_diff_key(app, key)),
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
                    let total = visible_anchor_count(&app.anchors, &app.filter);
                    if total > 0 {
                        feed.list.select(Some(0));
                    } else {
                        feed.list.select(None);
                    }
                }
                KeyCode::Enter => {
                    let total = visible_anchor_count(&app.anchors, &app.filter);
                    if total > 0 {
                        feed.filter_input_open = false;
                    }
                    // 0 results: stay in filter so user can keep typing
                }
                KeyCode::Backspace => {
                    app.filter.pop();
                    let total = visible_anchor_count(&app.anchors, &app.filter);
                    if total > 0 {
                        feed.list.select(Some(0));
                    } else {
                        feed.list.select(None);
                    }
                }
                KeyCode::Up => {
                    let total = visible_anchor_count(&app.anchors, &app.filter) as i32;
                    if total > 0 {
                        let cur = feed.list.selected().unwrap_or(0) as i32;
                        feed.list.select(Some((cur - 1).max(0) as usize));
                    }
                }
                KeyCode::Down => {
                    let total = visible_anchor_count(&app.anchors, &app.filter) as i32;
                    if total > 0 {
                        let cur = feed.list.selected().unwrap_or(0) as i32;
                        feed.list.select(Some(((cur + 1).min(total - 1)) as usize));
                    }
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.filter.push(c);
                    let total = visible_anchor_count(&app.anchors, &app.filter);
                    if total > 0 {
                        feed.list.select(Some(0));
                    } else {
                        feed.list.select(None);
                    }
                }
                _ => {}
            }
        }
        return Ok(false);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('f' | '/'))
    {
        if let Some(View::Feed(feed)) = app.stack.last_mut() {
            feed.filter_input_open = true;
            app.filter.clear();
        }
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
        KeyCode::Char('s') => {
            let ss = SearchState {
                input: String::new(),
                query: String::new(),
                answer: None,
                results: Vec::new(),
                list: ListState::default(),
                status: SearchStatus::Input,
                rx: None,
            };
            app.stack.push(View::Search(ss));
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
        KeyCode::Char('L') => goto_selected(terminal, app)?,
        KeyCode::Char('d') => {
            if let Some(anchor) = app.selected_anchor() {
                if anchor.kind == MemoryKind::Anchor {
                    open_diff_view(app);
                } else {
                    app.flash("this point has no git commit yet; press enter to inspect memory");
                }
            }
        }
        KeyCode::Char('b') => {
            if app
                .selected_anchor()
                .is_some_and(|a| a.kind == MemoryKind::Anchor)
            {
                open_blame_picker(app);
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
        KeyCode::Enter => open_selected_memory(app),
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

// ── Search view keys ──────────────────────────────────────────────────

fn handle_search_key(app: &mut App, key: KeyEvent) -> bool {
    let is_input = matches!(
        app.stack.last(),
        Some(View::Search(SearchState {
            status: SearchStatus::Input,
            ..
        }))
    );

    if is_input {
        match key.code {
            KeyCode::Esc => {
                app.stack.pop();
            }
            KeyCode::Enter => {
                let query = match app.stack.last() {
                    Some(View::Search(ss)) => {
                        let q = ss.input.trim().to_string();
                        if q.is_empty() {
                            None
                        } else {
                            Some(q)
                        }
                    }
                    _ => None,
                };
                if let Some(q) = query {
                    let new_ss = app.start_search(q);
                    if let Some(View::Search(ss)) = app.stack.last_mut() {
                        *ss = new_ss;
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(View::Search(ss)) = app.stack.last_mut() {
                    ss.input.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(View::Search(ss)) = app.stack.last_mut() {
                    ss.input.push(c);
                }
            }
            _ => {}
        }
        return false;
    }

    if let Some(View::Search(ss)) = app.stack.last_mut() {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc => {
                app.stack.pop();
            }
            KeyCode::Char('s' | '/') => {
                ss.rx = None;
                ss.status = SearchStatus::Input;
                ss.input = ss.query.clone();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let n = ss.results.len() as i32;
                if n > 0 {
                    let cur = ss.list.selected().unwrap_or(0) as i32;
                    ss.list.select(Some((cur - 1).max(0) as usize));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = ss.results.len() as i32;
                if n > 0 {
                    let cur = ss.list.selected().unwrap_or(0) as i32;
                    ss.list.select(Some(((cur + 1).min(n - 1)) as usize));
                }
            }
            KeyCode::Enter => {
                let action = ss.list.selected().and_then(|idx| {
                    ss.results
                        .get(idx)
                        .map(|row| (row.anchor_sha.clone(), row.source.clone()))
                });
                if let Some((sha_opt, source)) = action {
                    if let Some(sha) = sha_opt {
                        if let Some(pos) = app.anchors.iter().position(|a| a.sha.starts_with(&sha))
                        {
                            app.stack.pop();
                            if let Some(View::Feed(feed)) = app.stack.first_mut() {
                                feed.list.select(Some(pos));
                            }
                        } else {
                            app.flash(format!("anchor {sha} not in local history"));
                        }
                    } else {
                        let label = if source == "memory" {
                            "memory hit"
                        } else {
                            "result"
                        };
                        app.flash(format!("{label} has no anchor to navigate to"));
                    }
                }
            }
            _ => {}
        }
    }
    false
}

// ── Code search view keys ─────────────────────────────────────────────────

fn handle_code_search_key(app: &mut App, key: KeyEvent) -> bool {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.stack.pop();
            true
        }
        _ => false,
    }
}

// ── Transcript view keys ──────────────────────────────────────────────

fn handle_transcript_key(app: &mut App, key: KeyEvent) -> bool {
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
            KeyCode::Char('t') => jump_to_tool_call(ts, 1),
            KeyCode::Char('T') => jump_to_tool_call(ts, -1),
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
    ts.tool_call_lines = compute_tool_call_lines(&ts.lines);
    ts.tool_call_cursor = 0;
    if !ts.filter.is_empty() {
        recompute_transcript_matches(ts);
    }
}

/// Scan rendered transcript lines for tool-call boundaries.
/// A tool call starts with a line whose first visible span contains "tool".
fn compute_tool_call_lines(lines: &[Line<'_>]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.spans.iter().any(|s| {
                let t = s.content.trim();
                t == "tool" || t == "tool_use" || t == "tool_result"
            })
        })
        .map(|(i, _)| i)
        .collect()
}

fn jump_to_tool_call(ts: &mut TranscriptState, delta: i32) {
    if ts.tool_call_lines.is_empty() {
        return;
    }
    let n = ts.tool_call_lines.len() as i32;
    let cur = ts.tool_call_cursor as i32;
    let next = (cur + delta).rem_euclid(n) as usize;
    ts.tool_call_cursor = next;
    ts.scroll = ts.tool_call_lines[next] as u16;
}

// ── Diff view keys ───────────────────────────────────────────────────

fn handle_diff_key(app: &mut App, key: KeyEvent) -> bool {
    if let Some(View::Diff(ds)) = app.stack.last_mut() {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc | KeyCode::Backspace => {
                app.stack.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ds.scroll = ds.scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ds.scroll = ds.scroll.saturating_add(1);
            }
            KeyCode::PageUp => {
                ds.scroll = ds.scroll.saturating_sub(20);
            }
            KeyCode::PageDown => {
                ds.scroll = ds.scroll.saturating_add(20);
            }
            KeyCode::Char('g') => {
                ds.scroll = 0;
            }
            KeyCode::Char('G') => {
                ds.scroll = ds.lines.len().saturating_sub(1) as u16;
            }
            _ => {}
        }
    }
    false
}

fn open_diff_view(app: &mut App) {
    let Some(anchor) = app.selected_anchor() else {
        return;
    };
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(&git)
        .args(["show", "--format=", "--stat", "--patch", &anchor.sha])
        .current_dir(&app.root)
        .output();
    match output {
        Ok(out) => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let lines = super::draw::parse_diff_lines(&raw);
            app.stack.push(View::Diff(DiffState {
                sha: anchor.sha,
                subject: anchor.subject,
                lines,
                scroll: 0,
            }));
        }
        Err(e) => {
            app.flash(format!("diff failed: {e}"));
        }
    }
}

// ── Picker view keys ──────────────────────────────────────────────────

fn handle_picker_key(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, String> {
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
            .map_or(PickerAction::Noop, |s| PickerAction::OpenSession {
                session: s.clone(),
                project_path: project_path.clone(),
                siblings: sessions.clone(),
                idx,
            }),
        PickerKind::BlameFile { files, sha, root } => {
            files
                .get(idx)
                .map_or(PickerAction::Noop, |f| PickerAction::Blame {
                    root: root.clone(),
                    file: f.clone(),
                    sha: sha.clone(),
                })
        }
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
            let tool_call_lines = compute_tool_call_lines(&lines);
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
                tool_call_lines,
                tool_call_cursor: 0,
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

pub(super) fn open_selected_memory_public(app: &mut App) {
    open_selected_memory(app);
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

fn goto_selected(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<(), String> {
    let Some(anchor) = app.selected_anchor() else {
        return Ok(());
    };
    let target = anchor.sha.clone();
    let root = app.root.clone();
    suspend_and_run(terminal, || run_oobo_goto(&root, &target))?;
    app.reload_anchors();
    app.flash("loaded — run `oobo back` to return");
    Ok(())
}

fn open_sessions_for_anchor_row(app: &mut App, anchor: &super::types::AnchorRow) {
    let sessions = load_sessions_for_anchor(&app.root, &anchor.sha);
    if sessions.is_empty() {
        app.flash("no sessions linked to this anchor");
        return;
    }
    if sessions.len() == 1 {
        let session = sessions.into_iter().next().unwrap();
        let lines = load_transcript_lines(&app.root, &session);
        let tool_call_lines = compute_tool_call_lines(&lines);
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
            tool_call_lines,
            tool_call_cursor: 0,
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

fn open_shadow_memory(app: &mut App, shadow: &super::types::AnchorRow) {
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
                "oobo memory",
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
                "Press g to goto this point, or run `oobo goto <id>` from the terminal.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
    }
    let tool_call_lines = compute_tool_call_lines(&lines);
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
        tool_call_lines,
        tool_call_cursor: 0,
    }));
}

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
