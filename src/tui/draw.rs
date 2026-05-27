use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use super::app::{visible_anchor_count, visible_anchors, App};
use super::format::{pad_or_truncate, relative_time};
use super::types::{DiffState, FeedState, MemoryKind, SearchState, SearchStatus, View};

// ── Loading skeleton ──────────────────────────────────────────────────

pub(super) struct LoadingSkeleton<'a> {
    pub(super) project_name: &'a str,
    pub(super) enabled: bool,
    pub(super) branch: Option<&'a str>,
}

pub(super) fn draw_loading_skeleton(
    frame: &mut ratatui::Frame<'_>,
    sk: &LoadingSkeleton<'_>,
    spinner: &str,
) {
    let area = frame.area();

    let notice = !sk.enabled;
    let constraints = if notice {
        vec![
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(3),
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

    let branch_str = sk.branch.unwrap_or("detached");
    let sep = Span::styled(" · ", Style::default().fg(Color::DarkGray));

    let tracking_span = if sk.enabled {
        Span::styled("on", Style::default().fg(Color::Green))
    } else {
        Span::styled("off", Style::default().fg(Color::Red))
    };

    let header_lines = vec![
        Line::from(vec![
            Span::styled("  ⚓ ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "oobo",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                sk.project_name.to_string(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(branch_str.to_string(), Style::default().fg(Color::DarkGray)),
            sep.clone(),
            Span::styled(
                format!("{spinner} loading"),
                Style::default().fg(Color::DarkGray),
            ),
            sep,
            tracking_span,
        ]),
    ];
    frame.render_widget(Paragraph::new(header_lines), header_area);

    if let Some(na) = notice_area {
        draw_tracking_notice(frame, na);
    }

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

    let footer = Line::from(vec![
        Span::styled(" /", Style::default().fg(Color::White)),
        Span::styled(" filter  ", Style::default().fg(Color::DarkGray)),
        Span::styled("s", Style::default().fg(Color::White)),
        Span::styled(" search  ", Style::default().fg(Color::DarkGray)),
        Span::styled("enter", Style::default().fg(Color::White)),
        Span::styled(" open  ", Style::default().fg(Color::DarkGray)),
        Span::styled("?", Style::default().fg(Color::White)),
        Span::styled(" help  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::White)),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(footer), footer_area);
}

// ── Main draw dispatcher ──────────────────────────────────────────────

pub(super) fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    if let Some(first) = app.stack.first() {
        match first {
            View::Feed(feed) => draw_feed(frame, app, feed),
            View::Search(ss) => draw_search(frame, app, ss),
            View::CodeSearch { query, results } => draw_code_search(frame, query, results),
            View::Transcript(ts) => super::transcript::draw_transcript(frame, ts),
            View::Diff(ds) => draw_diff(frame, ds),
            View::Picker(_) | View::Help => {}
        }
    }

    if app.stack.len() > 1 {
        if let Some(top) = app.stack.last() {
            match top {
                View::Feed(_) => {}
                View::Search(ss) => draw_search(frame, app, ss),
                View::CodeSearch { query, results } => draw_code_search(frame, query, results),
                View::Transcript(ts) => super::transcript::draw_transcript(frame, ts),
                View::Diff(ds) => draw_diff(frame, ds),
                View::Picker(p) => super::detail::draw_picker_overlay(frame, p),
                View::Help => super::detail::draw_help_overlay(frame),
            }
        }
    } else if matches!(app.stack.last(), Some(View::Help)) {
        super::detail::draw_help_overlay(frame);
    }
}

// ── Feed view ─────────────────────────────────────────────────────────

fn draw_feed(frame: &mut ratatui::Frame<'_>, app: &App, feed: &FeedState) {
    let area = frame.area();
    let notice = !app.enabled;
    let constraints = if notice {
        vec![
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(3),
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

    let wide = body_area.width >= 80;
    if wide {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
            .split(body_area);
        draw_anchor_list(frame, app, feed, body[0]);
        super::detail::draw_anchor_detail(frame, app, body[1]);
    } else {
        draw_anchor_list(frame, app, feed, body_area);
    }
    draw_footer(frame, app, feed, footer_area);
}

fn draw_tracking_notice(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            "  tracking off",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " — new sessions won't be captured on commit. press ",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            "e",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to enable", Style::default().fg(Color::Yellow)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_empty_state(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let hints: Vec<Line<'static>> = if app.enabled {
        vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                format!("  no memory yet for \"{}\"", app.project_name),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  oobo captures working memory as you go and makes it durable when you commit.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  · make a commit:   git commit -m \"your message\"",
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
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                format!("  oobo is not tracking \"{}\"", app.project_name),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  oobo captures AI sessions and links them to your commits.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  enable tracking to start building memory for this project.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("  · press ", Style::default().fg(Color::Gray)),
                Span::styled(
                    "e",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to enable tracking now", Style::default().fg(Color::Gray)),
            ]),
            Line::from(Span::styled(
                "  · or run:  oobo enable",
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

// ── Header ────────────────────────────────────────────────────────────

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

    let sep = Span::styled(" · ", Style::default().fg(Color::DarkGray));

    let tracking_span = if app.enabled {
        Span::styled("on", Style::default().fg(Color::Green))
    } else {
        Span::styled("off", Style::default().fg(Color::Red))
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("  ⚓ ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "oobo",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.project_name.clone(), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(branch.to_string(), Style::default().fg(Color::DarkGray)),
            sep.clone(),
            Span::styled(count_str, Style::default().fg(Color::DarkGray)),
            sep,
            tracking_span,
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Left), area);
}

pub(super) fn chip(label: &str, value: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!("{label} {value}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

// ── Anchor list ───────────────────────────────────────────────────────

fn draw_anchor_list(frame: &mut ratatui::Frame<'_>, app: &App, feed: &FeedState, area: Rect) {
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
                        .map_or_else(|| "-".into(), super::format_tokens);
                    let sessions = if r.session_count > 0 {
                        format!("{}s", r.session_count)
                    } else {
                        "-".to_string()
                    };
                    format!("{tok_str:>7} {sessions:>3}")
                }
                MemoryKind::ShadowAnchor => {
                    let tid: String = r.sha.chars().take(8).collect();
                    let turn_label = format!("t:{tid}");
                    match &r.worktree_hint {
                        Some(wt) => format!("{turn_label}  wt:{wt}"),
                        None => turn_label,
                    }
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

// ── Footer ────────────────────────────────────────────────────────────

fn draw_footer(frame: &mut ratatui::Frame<'_>, app: &App, feed: &FeedState, area: Rect) {
    let content: Line<'static> = if feed.filter_input_open {
        Line::from(vec![
            Span::styled(
                " / ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.filter.clone()),
            Span::styled("▎", Style::default().fg(Color::Cyan)),
        ])
    } else if let Some(flash) = &app.flash {
        Line::from(Span::styled(
            format!(" {flash}"),
            Style::default().fg(Color::Yellow),
        ))
    } else if !app.filter.is_empty() {
        let count = visible_anchor_count(&app.anchors, &app.filter);
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Cyan)),
            Span::styled(app.filter.clone(), Style::default().fg(Color::White)),
            Span::styled(
                format!("  ({count} matches)"),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::White)),
            Span::styled(" filter  ", Style::default().fg(Color::DarkGray)),
            Span::styled("s", Style::default().fg(Color::White)),
            Span::styled(" search  ", Style::default().fg(Color::DarkGray)),
            Span::styled("enter", Style::default().fg(Color::White)),
            Span::styled(" open  ", Style::default().fg(Color::DarkGray)),
            Span::styled("?", Style::default().fg(Color::White)),
            Span::styled(" help  ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::White)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ])
    };
    frame.render_widget(Paragraph::new(content), area);
}

// ── Search view ──────────────────────────────────────────────────────

fn draw_search(frame: &mut ratatui::Frame<'_>, app: &App, ss: &SearchState) {
    use super::app::SPINNER;
    use ratatui::widgets::Clear;

    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Reset)),
        area,
    );

    let notice = !app.enabled;
    let constraints = if notice {
        vec![
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(3),
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

    let branch = app.branch.as_deref().unwrap_or("detached");
    let sep = Span::styled(" · ", Style::default().fg(Color::DarkGray));

    let tracking_span = if app.enabled {
        Span::styled("on", Style::default().fg(Color::Green))
    } else {
        Span::styled("off", Style::default().fg(Color::Red))
    };

    let status_str = match &ss.status {
        SearchStatus::Input => "search".to_string(),
        SearchStatus::Loading => {
            let s = SPINNER[app.tick % SPINNER.len()];
            format!("{s} searching")
        }
        SearchStatus::Done => {
            let n = ss.results.len();
            let mem = ss.results.iter().filter(|r| r.source == "memory").count();
            if mem > 0 {
                format!("{n} results ({mem} memories)")
            } else {
                format!("{n} results")
            }
        }
        SearchStatus::Error(_) => "search failed".to_string(),
        SearchStatus::NoApiKey => "no API key".to_string(),
    };

    let header = vec![
        Line::from(vec![
            Span::styled("  ⚓ ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "oobo",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.project_name.clone(), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(branch.to_string(), Style::default().fg(Color::DarkGray)),
            sep.clone(),
            Span::styled(status_str, Style::default().fg(Color::DarkGray)),
            sep,
            tracking_span,
        ]),
    ];
    frame.render_widget(
        Paragraph::new(header).alignment(Alignment::Left),
        header_area,
    );

    if let Some(na) = notice_area {
        draw_tracking_notice(frame, na);
    }

    let has_answer = ss.answer.is_some();
    if ss.results.is_empty() && !has_answer && !matches!(ss.status, SearchStatus::Loading) {
        let hint = match &ss.status {
            SearchStatus::Input => vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  type a question and press enter to search",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  examples:",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "    why did we build the auth middleware?",
                    Style::default().fg(Color::Gray),
                )),
                Line::from(Span::styled(
                    "    what decisions were made about caching?",
                    Style::default().fg(Color::Gray),
                )),
                Line::from(Span::styled(
                    "    who worked on the payment integration?",
                    Style::default().fg(Color::Gray),
                )),
            ],
            SearchStatus::Done => vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  no results for \"{}\"", ss.query),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  press s to search again",
                    Style::default().fg(Color::DarkGray),
                )),
            ],
            SearchStatus::Error(e) => vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  search failed: {e}"),
                    Style::default().fg(Color::Red),
                )),
            ],
            SearchStatus::NoApiKey => vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  search requires an API key",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  run: oobo settings set key <API_KEY>",
                    Style::default().fg(Color::Gray),
                )),
            ],
            SearchStatus::Loading => vec![],
        };
        frame.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }), body_area);
    } else if matches!(ss.status, SearchStatus::Loading) {
        let s = SPINNER[app.tick % SPINNER.len()];
        let loading = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(format!("  {s} "), Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("searching for \"{}\"…", ss.query),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(loading), body_area);
    } else {
        let (answer_area, list_area) = if let Some(answer) = ss.answer.as_deref() {
            let usable_width = body_area.width.saturating_sub(6) as usize;
            let text_lines = if usable_width > 0 {
                answer.len().div_ceil(usable_width)
            } else {
                1
            };
            let answer_height = (text_lines as u16) + 3; // blank + text + blank + separator
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(answer_height), Constraint::Min(1)])
                .split(body_area);
            (Some(split[0]), split[1])
        } else {
            (None, body_area)
        };

        if let Some((area, answer)) = answer_area.zip(ss.answer.as_deref()) {
            let answer_lines = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  💡 ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        answer.to_string(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "  ─".to_string() + &"─".repeat(area.width.saturating_sub(4) as usize),
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            frame.render_widget(
                Paragraph::new(answer_lines).wrap(Wrap { trim: false }),
                area,
            );
        }

        let items: Vec<ListItem> = ss
            .results
            .iter()
            .map(|r| {
                let is_memory = r.source == "memory";
                let when = r
                    .timestamp
                    .map_or_else(|| "-".to_string(), super::format::relative_time);
                let sha = r.anchor_sha.as_deref().unwrap_or("-");

                let max_snippet = (list_area.width as usize).saturating_sub(6);
                let snippet: String = r.snippet.chars().take(max_snippet).collect();

                let score_str = format!("{:.0}%", r.score * 100.0);
                let tok_str = r
                    .tokens
                    .filter(|t| *t > 0)
                    .map_or_else(String::new, |t| format!(" {}tok", super::format_tokens(t)));

                let mut spans = vec![
                    if is_memory {
                        Span::styled(" ◆ ", Style::default().fg(Color::Magenta))
                    } else {
                        Span::styled(" ● ", Style::default().fg(Color::Green))
                    },
                    Span::styled(
                        format!("{sha:<8}"),
                        Style::default().fg(if is_memory {
                            Color::Magenta
                        } else {
                            Color::Yellow
                        }),
                    ),
                    Span::styled(format!(" {when:<5} "), Style::default().fg(Color::DarkGray)),
                ];

                if is_memory {
                    if let Some(author) = &r.author {
                        let short_author: String = author
                            .split_whitespace()
                            .next()
                            .unwrap_or(author)
                            .chars()
                            .take(10)
                            .collect();
                        spans.push(Span::styled(
                            format!("{short_author} "),
                            Style::default().fg(Color::Cyan),
                        ));
                    }
                } else if let Some(tool) = &r.tool {
                    spans.push(Span::styled(
                        format!("{tool} "),
                        Style::default().fg(Color::Blue),
                    ));
                }

                spans.push(Span::styled(
                    format!("{score_str}{tok_str}"),
                    Style::default().fg(Color::DarkGray),
                ));

                let line1 = Line::from(spans);
                let mut line2_spans = vec![Span::raw("   ")];
                if r.project_name != "remote" {
                    line2_spans.push(Span::styled(
                        format!("[{}] ", r.project_name),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                line2_spans.push(Span::styled(
                    snippet,
                    Style::default().fg(if is_memory { Color::White } else { Color::Gray }),
                ));
                let line2 = Line::from(line2_spans);

                ListItem::new(vec![line1, line2, Line::from("")])
            })
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
        let mut state = ss.list;
        frame.render_stateful_widget(list, list_area, &mut state);
    }

    let footer: Line<'static> = if matches!(ss.status, SearchStatus::Input) {
        Line::from(vec![
            Span::styled(" 🔍 ", Style::default().fg(Color::Cyan)),
            Span::raw(ss.input.clone()),
            Span::styled("▎", Style::default().fg(Color::Cyan)),
        ])
    } else if let Some(flash) = &app.flash {
        Line::from(Span::styled(
            format!(" {flash}"),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        Line::from(vec![
            Span::styled("s", Style::default().fg(Color::White)),
            Span::styled(" new search  ", Style::default().fg(Color::DarkGray)),
            Span::styled("enter", Style::default().fg(Color::White)),
            Span::styled(" jump to anchor  ", Style::default().fg(Color::DarkGray)),
            Span::styled("esc", Style::default().fg(Color::White)),
            Span::styled(" back  ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::White)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ])
    };
    frame.render_widget(Paragraph::new(footer), footer_area);
}

// ── Diff view ────────────────────────────────────────────────────────

fn draw_diff(frame: &mut ratatui::Frame<'_>, ds: &DiffState) {
    use ratatui::widgets::Clear;

    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Reset)),
        area,
    );

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let short: String = ds.sha.chars().take(8).collect();
    let header = vec![
        Line::from(vec![
            Span::styled(
                " diff",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {short}"), Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("  {}", ds.subject),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(Span::styled(
            " ─".to_string() + &"─".repeat(area.width.saturating_sub(2) as usize),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(header), layout[0]);

    let visible_height = layout[1].height;
    let total_lines = ds.lines.len() as u16;
    let scroll = ds.scroll.min(total_lines.saturating_sub(visible_height));
    let body = Paragraph::new(ds.lines.clone())
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(body, layout[1]);

    let pct = if total_lines == 0 {
        100
    } else {
        ((u32::from(scroll) + u32::from(visible_height)) * 100 / u32::from(total_lines)).min(100)
    };
    let footer = Line::from(vec![
        Span::styled(" esc", Style::default().fg(Color::White)),
        Span::styled(" back  ", Style::default().fg(Color::DarkGray)),
        Span::styled("j/k", Style::default().fg(Color::White)),
        Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{pct}%"), Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(footer), layout[2]);
}

/// Parse raw diff output into styled TUI lines.
pub(super) fn parse_diff_lines(raw: &str) -> Vec<Line<'static>> {
    raw.lines()
        .map(|line| {
            if line.starts_with('+') && !line.starts_with("+++") {
                Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::Green),
                ))
            } else if line.starts_with('-') && !line.starts_with("---") {
                Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::Red),
                ))
            } else if line.starts_with("@@") {
                Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::Cyan),
                ))
            } else if line.starts_with("diff --git") || line.starts_with("index ") {
                Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ))
            } else if line.starts_with("---") || line.starts_with("+++") {
                Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::DIM),
                ))
            } else {
                Line::from(Span::raw(format!("  {line}")))
            }
        })
        .collect()
}

// ── Code search (sonar) TUI ──────────────────────────────────────────────────

fn draw_code_search(
    frame: &mut ratatui::Frame,
    query: &str,
    results: &[sonar_core::types::SearchResult],
) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("search ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(format!("\"{}\" ", query)),
        Span::styled(
            format!("({} results)", results.len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, chunks[0]);

    if results.is_empty() {
        let empty = Paragraph::new("No results found. Try a different query or run `oobo search --content all`.")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(empty, chunks[1]);
        return;
    }

    let items: Vec<ListItem> = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let lang = r.chunk.language.as_deref().unwrap_or("?");
            let header_line = Line::from(vec![
                Span::styled(
                    format!("{:>2}. ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    &r.chunk.file_path,
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(":{}-{}", r.chunk.start_line, r.chunk.end_line)),
                Span::styled(format!(" [{lang}]"), Style::default().fg(Color::Blue)),
                Span::styled(
                    format!("  {:.3}", r.score),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            let snippet: String = r
                .chunk
                .content
                .lines()
                .take(3)
                .map(|l| format!("    {l}"))
                .collect::<Vec<_>>()
                .join("\n");

            let snippet_line = Line::from(Span::styled(
                snippet,
                Style::default().fg(Color::White),
            ));

            ListItem::new(vec![header_line, snippet_line, Line::raw("")])
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Code Results ")
            .borders(Borders::ALL),
    );
    frame.render_widget(list, chunks[1]);
}
