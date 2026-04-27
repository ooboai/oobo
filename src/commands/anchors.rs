//! `oobo anchors` — the flagship view.
//!
//! List anchors (in or out of a repo) and drill into a single anchor via
//! `anchors show <sha>`. See `tests/cli-spec/02-anchors.md` for the exact
//! output contract.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::anchor::Anchor;
use crate::core::turn::TurnSnapshot;
use std::collections::HashMap;

/// User-facing filters for `anchors` list mode.
#[derive(Debug, Default, Clone)]
pub struct Options {
    pub limit: usize,
    pub since: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
}

// ------------------------------------------------------------------
// LIST
// ------------------------------------------------------------------

/// `oobo anchors` — list recent anchors.
pub fn run_list(cfg: &Config, opts: Options, mode: OutputMode) -> Result<i32, String> {
    let in_repo = crate::git::proxy::project_root(cfg).is_some();

    if in_repo && opts.project.is_some() {
        eprintln!("error: --project is not allowed inside a repo (current project is implied)");
        return Ok(2);
    }

    if in_repo
        && mode == OutputMode::Tui
        && opts.limit == 50
        && opts.since.is_none()
        && opts.tool.is_none()
    {
        return crate::tui::app::run(cfg);
    }

    let since_epoch = match opts.since.as_deref() {
        Some(raw) => match parse_since(raw) {
            Ok(ts) => Some(ts),
            Err(e) => {
                eprintln!("error: invalid --since '{raw}': {e}");
                return Ok(2);
            }
        },
        None => None,
    };

    let rows = if in_repo {
        load_in_repo(cfg, &opts, since_epoch)?
    } else {
        eprintln!("anchor: cross-project listing requires being inside a git repository.");
        Vec::new()
    };

    match mode {
        OutputMode::Json => emit_list_json(cfg, &rows, in_repo),
        OutputMode::Agent => emit_list_agent(&rows, in_repo),
        OutputMode::Tui => emit_list_pretty(&rows, in_repo),
    }
    Ok(0)
}

// ------------------------------------------------------------------
// SHOW
// ------------------------------------------------------------------

/// `oobo anchors show <sha>` — drill-down on one anchor.
pub fn run_show(cfg: &Config, sha: &str, mode: OutputMode) -> Result<i32, String> {
    let root = crate::git::proxy::project_root(cfg)
        .ok_or_else(|| "not inside a git repository".to_string())?;

    let matches = resolve_sha(&root, sha)?;
    match matches.len() {
        0 => {
            eprintln!("error: no anchor found for '{sha}'");
            return Ok(1);
        }
        1 => {}
        _ => {
            eprintln!("error: '{sha}' matches multiple anchors:");
            for (full, subj) in &matches {
                eprintln!("  {}  {}", &full[..7.min(full.len())], subj);
            }
            return Ok(1);
        }
    }
    let commit_hash = matches[0].0.clone();
    let anchor = load_anchor(&root, &commit_hash)
        .ok_or_else(|| format!("anchor row missing for {commit_hash}"))?;

    let sessions = load_sessions(&root, &commit_hash);

    match mode {
        OutputMode::Json => emit_show_json(cfg, &anchor, &sessions),
        OutputMode::Agent => emit_show_agent(&anchor, &sessions),
        OutputMode::Tui => emit_show_pretty(&anchor, &sessions),
    }
    Ok(0)
}

// ------------------------------------------------------------------
// row model
// ------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Row {
    kind: RowKind,
    project_name: String,
    id: String,
    subject: String,
    timestamp: i64,
    tool: Option<String>,
    tokens: i64,
    session_count: usize,
    ai_pct: Option<i64>,
    files: usize,
    tool_calls: usize,
    session_id: Option<String>,
    turn_index: Option<i64>,
    parent_anchor: Option<String>,
    restored_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Anchor,
    ShadowAnchor,
}

impl RowKind {
    fn agent_label(self) -> &'static str {
        match self {
            RowKind::Anchor => "anchor",
            RowKind::ShadowAnchor => "shadow",
        }
    }

    fn json_label(self) -> &'static str {
        match self {
            RowKind::Anchor => "anchor",
            RowKind::ShadowAnchor => "shadow_anchor",
        }
    }
}

// ------------------------------------------------------------------
// loaders
// ------------------------------------------------------------------

fn load_in_repo(
    cfg: &Config,
    opts: &Options,
    since: Option<i64>,
) -> Result<Vec<Row>, String> {
    let n = opts.limit.max(1);
    let log = crate::git::proxy::run_git_capture(
        cfg,
        &[
            "log",
            &format!("-{}", n * 4),
            "--format=%H|||%s|||%ct",
        ],
    )
    .unwrap_or_default();

    let root = crate::git::proxy::project_root(cfg).unwrap_or_default();

    let project_name = std::path::Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "-".to_string());

    let (all_anchors, all_links) =
        crate::git::anchor_cache::load_anchors_cached(&root);
    let anchor_map: HashMap<String, &Anchor> = all_anchors
        .iter()
        .map(|a| (a.commit_hash.clone(), a))
        .collect();

    let mut out: Vec<Row> = Vec::new();
    for line in log.lines() {
        let parts: Vec<&str> = line.splitn(3, "|||").collect();
        if parts.len() < 3 {
            continue;
        }
        let sha = parts[0].to_string();
        let subject = parts[1].to_string();
        let ts: i64 = parts[2].parse().unwrap_or(0);

        if let Some(s) = since {
            if ts < s {
                continue;
            }
        }

        let (tool, tokens, count) = if let Some(_anchor) = anchor_map.get(&sha) {
            let links = all_links.get(&sha).cloned().unwrap_or_default();
            summarize_session_links(&links)
        } else {
            (None, 0, 0)
        };

        if let Some(t) = opts.tool.as_deref() {
            let want = t.to_lowercase();
            let has = tool
                .as_deref()
                .map(|x| x.to_lowercase())
                .unwrap_or_default();
            if has != want {
                continue;
            }
        }

        let ai_pct = anchor_map
            .get(&sha)
            .and_then(|a| a.ai_percentage)
            .map(|p| p.round() as i64);

        out.push(Row {
            kind: RowKind::Anchor,
            project_name: project_name.clone(),
            id: sha,
            subject,
            timestamp: ts,
            tool,
            tokens,
            session_count: count,
            ai_pct,
            files: 0,
            tool_calls: 0,
            session_id: None,
            turn_index: None,
            parent_anchor: None,
            restored_from: None,
        });
    }
    if !root.is_empty() {
        let parents = build_shadow_parents_from_cached(&all_anchors);
        out.extend(load_turn_rows(&root, &project_name, opts, since, &parents));
    }
    sort_memory_rows(&mut out);
    out.truncate(opts.limit);
    Ok(out)
}

fn load_turn_rows(
    project_root: &str,
    project_name: &str,
    opts: &Options,
    since: Option<i64>,
    parents: &HashMap<String, String>,
) -> Vec<Row> {
    crate::git::turns::list_turn_snapshots(project_root)
        .into_iter()
        .filter_map(|turn| turn_to_row(turn, project_name, opts, since, parents))
        .collect()
}

fn turn_to_row(
    turn: TurnSnapshot,
    project_name: &str,
    opts: &Options,
    since: Option<i64>,
    parents: &HashMap<String, String>,
) -> Option<Row> {
    let ts = turn.ended_at.or(turn.started_at).unwrap_or(turn.created_at);
    if let Some(s) = since {
        if ts < s {
            return None;
        }
    }
    if let Some(t) = opts.tool.as_deref() {
        if !turn.source.eq_ignore_ascii_case(t) {
            return None;
        }
    }
    Some(Row {
        kind: RowKind::ShadowAnchor,
        project_name: project_name.to_string(),
        id: turn.id.clone(),
        subject: turn_subject(&turn),
        timestamp: ts,
        tool: Some(turn.source.clone()),
        tokens: 0,
        session_count: 1,
        ai_pct: None,
        files: turn_file_count(&turn),
        tool_calls: turn.memory.tool_calls.len(),
        session_id: Some(turn.session_id.clone()),
        turn_index: Some(turn.turn_index),
        parent_anchor: parents.get(&turn.id).cloned(),
        restored_from: turn.restored_from.clone(),
    })
}

fn turn_subject(turn: &TurnSnapshot) -> String {
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

fn turn_file_count(turn: &TurnSnapshot) -> usize {
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
    all_anchors: &[Anchor],
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

fn sort_memory_rows(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        if a.parent_anchor.as_deref() == Some(b.id.as_str()) {
            return std::cmp::Ordering::Greater;
        }
        if b.parent_anchor.as_deref() == Some(a.id.as_str()) {
            return std::cmp::Ordering::Less;
        }
        if a.parent_anchor.is_some() && a.parent_anchor == b.parent_anchor {
            return a
                .turn_index
                .cmp(&b.turn_index)
                .then_with(|| a.id.cmp(&b.id));
        }
        b.timestamp.cmp(&a.timestamp).then_with(|| b.id.cmp(&a.id))
    });
}

fn summarize_session_links(
    links: &[crate::core::anchor::SessionLink],
) -> (Option<String>, i64, usize) {
    if links.is_empty() {
        return (None, 0, 0);
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
    (tool, total, links.len())
}

fn load_anchor(project_root: &str, commit_hash: &str) -> Option<Anchor> {
    crate::git::orphan::read_anchor(project_root, commit_hash)
}

#[derive(Debug, serde::Serialize)]
struct SessionInfo {
    id: String,
    tool: String,
    model: Option<String>,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    total: i64,
}

fn load_sessions(project_root: &str, commit_hash: &str) -> Vec<SessionInfo> {
    let links = crate::git::orphan::read_session_links(project_root, commit_hash);
    links.iter().map(|l| {
        let input = l.input_tokens.unwrap_or(0) as i64;
        let output = l.output_tokens.unwrap_or(0) as i64;
        let cache_read = l.cache_read_tokens.unwrap_or(0) as i64;
        let cache_write = l.cache_creation_tokens.unwrap_or(0) as i64;
        let total = input + output + cache_read + cache_write;
        SessionInfo {
            id: l.session_id.clone(),
            tool: l.agent.clone(),
            model: l.model.clone(),
            input, output, cache_read, cache_write, total,
        }
    }).collect()
}

// ------------------------------------------------------------------
// SHA resolution
// ------------------------------------------------------------------

fn resolve_sha(project_root: &str, prefix: &str) -> Result<Vec<(String, String)>, String> {
    let hashes = crate::git::orphan::list_anchor_hashes(project_root);
    let matches: Vec<(String, String)> = hashes.into_iter()
        .filter(|h| h.starts_with(prefix))
        .filter_map(|h| {
            let anchor = crate::git::orphan::read_anchor(project_root, &h)?;
            Some((h, anchor.message))
        })
        .collect();
    Ok(matches)
}

// ------------------------------------------------------------------
// emitters — list
// ------------------------------------------------------------------

fn emit_list_agent(rows: &[Row], in_repo: bool) {
    for r in rows {
        let id = short_id(&r.id);
        let rel = relative_time(r.timestamp);
        let subject = truncate_fixed(&r.subject, 40);
        let tool = r.tool.as_deref().unwrap_or("-");
        let tokens = if r.tokens > 0 {
            human_tokens(r.tokens)
        } else {
            "-".to_string()
        };
        let count = match r.kind {
            RowKind::Anchor if r.session_count > 0 => format!("{}s", r.session_count),
            RowKind::ShadowAnchor => format!("{}f/{}t", r.files, r.tool_calls),
            RowKind::Anchor => "-".to_string(),
        };
        let kind = r.kind.agent_label();
        if in_repo {
            println!("{kind:<6} {id:<10} {rel:<4} {subject:<40} {tool:<8} {tokens:<4} {count}",);
        } else {
            let proj = truncate_fixed(&r.project_name, 14);
            println!("{proj:<14} {kind:<6} {id:<10} {rel:<4} {subject:<40} {tool:<8} {tokens:<4} {count}",);
        }
    }
}

fn emit_list_pretty(rows: &[Row], in_repo: bool) {
    if rows.is_empty() {
        println!("No anchors yet. Commit through anchor to start anchoring sessions.");
        return;
    }
    println!("\x1b[1manchor memory\x1b[0m");
    println!("\x1b[2manchors are committed memory; local anchors are restorable points\x1b[0m");
    println!();
    for (idx, r) in rows.iter().enumerate() {
        let id = short_id(&r.id);
        let rel = relative_time(r.timestamp);
        let subject = truncate_display(&r.subject, 72);
        let tool = r.tool.as_deref().unwrap_or("-");
        let tokens = if r.tokens > 0 {
            human_tokens(r.tokens)
        } else {
            "-".to_string()
        };
        let (dot, kind, id_color, meta) = match r.kind {
            RowKind::Anchor => {
                let sessions = match r.session_count {
                    0 => "no linked sessions".to_string(),
                    1 => "1 session".to_string(),
                    n => format!("{n} sessions"),
                };
                (
                    "\x1b[32m●\x1b[0m",
                    "\x1b[32manchor\x1b[0m",
                    "\x1b[33m",
                    format!("{tool} · {tokens} · {sessions}"),
                )
            }
            RowKind::ShadowAnchor => {
                let idx = r
                    .turn_index
                    .map(|i| format!("snapshot {i}"))
                    .unwrap_or_else(|| "snapshot".to_string());
                let parent = r
                    .parent_anchor
                    .as_deref()
                    .map(|sha| format!(" · anchored under {}", short_sha(sha)))
                    .unwrap_or_default();
                (
                    "\x1b[2m○\x1b[0m",
                    "\x1b[2manchor\x1b[0m",
                    "\x1b[2m",
                    format!(
                        "{tool} · {idx} · {} file{} · {} tool{}{parent}",
                        r.files,
                        plural(r.files),
                        r.tool_calls,
                        plural(r.tool_calls)
                    ),
                )
            }
        };
        let ai_pct = r
            .ai_pct
            .map(|p| format!(" · \x1b[35m{p}% AI\x1b[0m"))
            .unwrap_or_default();
        if in_repo {
            println!(
                " {dot} \x1b[2m{rel:<5}\x1b[0m {kind:<20} {id_color}{id:<10}\x1b[0m  \x1b[1m{subject}\x1b[0m",
            );
            println!("   \x1b[2m{meta}{ai_pct}\x1b[0m");
        } else {
            let proj = truncate_fixed(&r.project_name, 16);
            println!(
                " {dot} \x1b[34m{proj:<16}\x1b[0m \x1b[2m{rel:<5}\x1b[0m {kind:<20} {id_color}{id:<10}\x1b[0m  \x1b[1m{subject}\x1b[0m",
            );
            println!("   \x1b[2m{meta}{ai_pct}\x1b[0m");
        }
        if idx + 1 < rows.len() {
            println!("   \x1b[2m│\x1b[0m");
        }
    }
}

fn emit_list_json(_cfg: &Config, rows: &[Row], in_repo: bool) {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut obj = serde_json::json!({
                "type": r.kind.json_label(),
                "id": r.id,
                "timestamp": chrono::DateTime::from_timestamp(r.timestamp, 0)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default(),
                "subject": r.subject,
                "tools": r.tool.clone().map(|t| vec![t]).unwrap_or_default(),
                "tokens": { "total": r.tokens },
                "sessions_count": r.session_count,
                "ai_pct": r.ai_pct,
            });
            if !in_repo {
                obj["project"] = serde_json::Value::String(r.project_name.clone());
            }
            match r.kind {
                RowKind::Anchor => {
                    obj["sha"] = serde_json::Value::String(r.id.clone());
                }
                RowKind::ShadowAnchor => {
                    obj["turn_id"] = serde_json::Value::String(r.id.clone());
                    obj["shadow_anchor_id"] = serde_json::Value::String(r.id.clone());
                    obj["files"] = serde_json::Value::Number(r.files.into());
                    obj["tool_calls"] = serde_json::Value::Number(r.tool_calls.into());
                    if let Some(session_id) = &r.session_id {
                        obj["session_id"] = serde_json::Value::String(session_id.clone());
                    }
                    if let Some(turn_index) = r.turn_index {
                        obj["turn_index"] = serde_json::Value::Number(turn_index.into());
                    }
                    if let Some(parent) = &r.parent_anchor {
                        obj["parent_anchor"] = serde_json::Value::String(parent.clone());
                    }
                    if let Some(restored_from) = &r.restored_from {
                        obj["restored_from"] = serde_json::Value::String(restored_from.clone());
                    }
                }
            }
            obj
        })
        .collect();
    crate::utils::print_json(&serde_json::Value::Array(arr));
}

// ------------------------------------------------------------------
// emitters — show
// ------------------------------------------------------------------

fn emit_show_agent(anchor: &Anchor, sessions: &[SessionInfo]) {
    let ts = chrono::DateTime::from_timestamp(anchor.committed_at, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let tool = sessions.first().map(|s| s.tool.as_str()).unwrap_or("-");
    let total: i64 = sessions.iter().map(|s| s.total).sum();
    let input: i64 = sessions.iter().map(|s| s.input).sum();
    let output: i64 = sessions.iter().map(|s| s.output).sum();
    let cache: i64 = sessions.iter().map(|s| s.cache_read + s.cache_write).sum();

    println!("sha:        {}", anchor.commit_hash);
    println!("subject:    {}", anchor.message);
    if !anchor.author.is_empty() {
        println!("author:     {}", anchor.author);
    }
    println!("timestamp:  {ts}");
    println!("tools:      {tool}");
    println!("tokens:     {total} (in {input} / out {output} / cache {cache})");
    if let Some(p) = anchor.ai_percentage {
        println!("ai_pct:     {:.0}", p);
    }
    if !sessions.is_empty() {
        println!("sessions:");
        for s in sessions {
            println!(
                "  {id} {tool} {total}",
                id = s.id,
                tool = s.tool,
                total = s.total,
            );
        }
    }
}

fn emit_show_pretty(anchor: &Anchor, sessions: &[SessionInfo]) {
    let sha7 = short_sha(&anchor.commit_hash);
    let rel = relative_time(anchor.committed_at);
    println!("\x1b[1m{sha7}\x1b[0m — {subj}", subj = anchor.message);
    if !anchor.author.is_empty() {
        println!(
            "\x1b[2m{author}  ·  {rel} ago\x1b[0m",
            author = anchor.author
        );
    }
    println!("─────────────────────────────────────────────────────────");
    let total: i64 = sessions.iter().map(|s| s.total).sum();
    let input: i64 = sessions.iter().map(|s| s.input).sum();
    let output: i64 = sessions.iter().map(|s| s.output).sum();
    let cache: i64 = sessions.iter().map(|s| s.cache_read + s.cache_write).sum();
    let tools: Vec<String> = sessions.iter().map(|s| s.tool.clone()).collect();
    println!(
        "TOOLS     {}",
        if tools.is_empty() {
            "-".to_string()
        } else {
            tools.join(", ")
        }
    );
    println!(
        "TOKENS    {total} (input {}, output {}, cache {})",
        human_tokens(input),
        human_tokens(output),
        human_tokens(cache)
    );
    if let Some(p) = anchor.ai_percentage {
        println!(
            "ATTRIB    {} AI lines · {} human lines · {:.0}% AI",
            anchor.ai_added, anchor.human_added, p
        );
    }
    if !sessions.is_empty() {
        println!();
        println!("SESSIONS");
        for s in sessions {
            let model = s.model.as_deref().unwrap_or("-");
            println!(
                "  \x1b[35m●\x1b[0m {tool} · {model} · {total} tokens",
                tool = s.tool,
                total = human_tokens(s.total),
            );
        }
    }
}

fn emit_show_json(cfg: &Config, anchor: &Anchor, sessions: &[SessionInfo]) {
    let ts = chrono::DateTime::from_timestamp(anchor.committed_at, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let parents = load_parent_hashes(cfg, &anchor.commit_hash);
    let sess_json: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "tool": s.tool,
                "model": s.model,
                "tokens": {
                    "input": s.input,
                    "output": s.output,
                    "cache_read": s.cache_read,
                    "cache_write": s.cache_write,
                    "total": s.total,
                }
            })
        })
        .collect();
    let total: i64 = sessions.iter().map(|s| s.total).sum();
    let input: i64 = sessions.iter().map(|s| s.input).sum();
    let output: i64 = sessions.iter().map(|s| s.output).sum();
    let cache_read: i64 = sessions.iter().map(|s| s.cache_read).sum();
    let cache_write: i64 = sessions.iter().map(|s| s.cache_write).sum();
    let json = serde_json::json!({
        "sha": anchor.commit_hash,
        "parents": parents,
        "timestamp": ts,
        "author": { "raw": anchor.author },
        "subject": anchor.message,
        "tools": sessions.iter().map(|s| s.tool.clone()).collect::<Vec<_>>(),
        "tokens": {
            "input": input,
            "output": output,
            "cache_read": cache_read,
            "cache_write": cache_write,
            "total": total,
        },
        "attribution": {
            "ai_lines": anchor.ai_added,
            "human_lines": anchor.human_added,
            "ai_pct": anchor.ai_percentage,
        },
        "sessions": sess_json,
        "shadow_anchors": anchor.turns,
        "files_changed": anchor.files_changed,
    });
    crate::utils::print_json(&json);
}

// ------------------------------------------------------------------
// helpers
// ------------------------------------------------------------------

fn short_sha(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}

fn short_id(id: &str) -> String {
    if id.starts_with('t') {
        id.chars().take(10).collect()
    } else {
        short_sha(id)
    }
}

fn truncate_fixed(s: &str, n: usize) -> String {
    let mut t: String = s.chars().take(n).collect();
    while t.chars().count() < n {
        t.push(' ');
    }
    t
}

fn truncate_display(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(3)).collect();
        t.push_str("...");
        t
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn human_tokens(n: i64) -> String {
    crate::utils::human_tokens(n)
}

fn relative_time(ts: i64) -> String {
    crate::utils::relative_time(ts)
}

fn parse_since(raw: &str) -> Result<i64, String> {
    crate::utils::parse_since(raw)
}

fn load_parent_hashes(cfg: &Config, commit_hash: &str) -> Vec<String> {
    let root = crate::git::proxy::project_root(cfg);
    let Some(root) = root else { return Vec::new() };
    crate::git::proxy::run_git_capture_in(
        cfg,
        &["rev-list", "--parents", "-n", "1", commit_hash],
        Some(&root),
    )
    .map(|line| parse_parent_hashes(&line))
    .unwrap_or_default()
}

fn parse_parent_hashes(line: &str) -> Vec<String> {
    line.split_whitespace()
        .skip(1)
        .map(|s| s.to_string())
        .collect()
}

// ------------------------------------------------------------------
// backward-compat shim used by `commands/bare.rs`
// ------------------------------------------------------------------

/// Kept for bare `oobo` in-repo agent/json modes (byte-for-byte equivalence).
pub fn run(cfg: &Config, limit: usize, mode: OutputMode) -> Result<(), String> {
    let opts = Options {
        limit,
        ..Default::default()
    };
    run_list(cfg, opts, mode)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_since_duration() {
        let now = chrono::Utc::now().timestamp();
        let t = parse_since("24h").unwrap();
        assert!((now - t - 24 * 3600).abs() < 5);
    }

    #[test]
    fn test_parse_since_iso() {
        let t = parse_since("2020-01-01T00:00:00Z").unwrap();
        assert_eq!(t, 1577836800);
    }

    #[test]
    fn test_parse_since_rejects_garbage() {
        assert!(parse_since("banana").is_err());
    }

    #[test]
    fn test_short_sha() {
        assert_eq!(short_sha("a1b2c3d4e5"), "a1b2c3d");
        assert_eq!(short_sha("abc"), "abc");
    }

    #[test]
    fn test_parse_parent_hashes_from_rev_list_line() {
        let parents = parse_parent_hashes("abc123 def456 789abc");
        assert_eq!(parents, vec!["def456".to_string(), "789abc".to_string()]);
    }

    #[test]
    fn test_human_tokens() {
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(1500), "1k");
        assert_eq!(human_tokens(2_500_000), "2.5M");
    }
}
