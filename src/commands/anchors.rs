//! `oobo anchors`  --  the flagship view.
//!
//! List anchors (in or out of a repo) and drill into a single anchor via
//! `anchors show <sha>`. See `tests/cli-spec/02-anchors.md` for the exact
//! output contract.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::anchor::Anchor;
use crate::error::{CliError, CmdResult};
use crate::feed::{FeedRow, RowKind};

/// User-facing filters for `anchors` list mode.
#[derive(Debug, Default, Clone)]
pub struct Options {
    pub limit: usize,
    pub since: Option<String>,
    pub tool: Option<String>,
}

// ------------------------------------------------------------------
// LIST
// ------------------------------------------------------------------

/// `oobo anchors`  --  list recent anchors.
#[tracing::instrument(skip_all)]
pub fn run_list(cfg: &Config, opts: &Options, mode: OutputMode) -> CmdResult {
    let root = crate::git::proxy::project_root(cfg);
    let in_repo = root.is_some();

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

    let rows = if let Some(ref root) = root {
        crate::feed::load(
            cfg,
            root,
            &crate::feed::LoadOptions {
                limit: opts.limit,
                since: since_epoch,
                tool: opts.tool.clone(),
            },
        )?
    } else {
        if mode == OutputMode::Json {
            let json = serde_json::json!({ "error": "not inside a git repository" });
            crate::utils::print_json(&json);
        } else {
            eprintln!("oobo: not inside a git repository.");
        }
        return Ok(1);
    };

    let project_name = root
        .as_deref()
        .and_then(|r| std::path::Path::new(r).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("all");

    match mode {
        OutputMode::Json => emit_list_json(cfg, &rows, project_name, in_repo),
        OutputMode::Agent => emit_list_agent(&rows, project_name, in_repo),
        OutputMode::Tui => emit_list_pretty(&rows, project_name, in_repo),
    }
    Ok(0)
}

// ------------------------------------------------------------------
// SHOW
// ------------------------------------------------------------------

/// `oobo anchors show <sha>`  --  drill-down on one anchor.
#[tracing::instrument(skip_all, fields(sha))]
pub fn run_show(cfg: &Config, sha: &str, mode: OutputMode) -> CmdResult {
    let root = crate::git::proxy::project_root(cfg).ok_or(CliError::NotARepo)?;

    let matches = resolve_sha(&root, sha);
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
    let v2_refs = load_v2_session_refs(&root, &commit_hash);

    match mode {
        OutputMode::Json => {
            emit_show_json(cfg, &anchor, &sessions, &v2_refs);
            Ok(0)
        }
        OutputMode::Agent => {
            emit_show_agent(&anchor, &sessions, &v2_refs);
            Ok(0)
        }
        OutputMode::Tui => crate::tui::app::run_show(cfg, &commit_hash),
    }
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

/// v2 session refs for an anchor, with passive pointer resolution —
/// foreign-home sessions show their pointer + hydration state, so the
/// listing in repo Y honestly reports sessions homed in repo X.
#[derive(Debug, serde::Serialize)]
pub(crate) struct V2SessionRefInfo {
    pub(crate) session_uid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) home_location: Option<String>,
    pub(crate) hydration: crate::git::orphan::v2::resolve::Hydration,
    pub(crate) turns_claimed: usize,
}

pub(crate) fn load_v2_session_refs(project_root: &str, commit_hash: &str) -> Vec<V2SessionRefInfo> {
    let repo_id = crate::project::id_for_root(project_root);
    let Some(record) = crate::git::orphan::v2::read_anchor(project_root, &repo_id, commit_hash)
    else {
        return Vec::new();
    };
    record
        .session_refs
        .iter()
        .map(|r| {
            let hydration = crate::git::orphan::v2::resolve::resolve_conversation_with(
                project_root,
                &repo_id,
                &r.session_uid,
                false,
            )
            .map_or(
                crate::git::orphan::v2::resolve::Hydration::StubOnly,
                |res| res.hydration,
            );
            V2SessionRefInfo {
                session_uid: r.session_uid.clone(),
                home_location: r.home_location.clone(),
                hydration,
                turns_claimed: r.turn_uids.len(),
            }
        })
        .collect()
}

fn load_sessions(project_root: &str, commit_hash: &str) -> Vec<SessionInfo> {
    let links = crate::git::orphan::read_session_links(project_root, commit_hash);
    links
        .iter()
        .map(|l| {
            let input = l.input_tokens.unwrap_or(0) as i64;
            let output = l.output_tokens.unwrap_or(0) as i64;
            let cache_read = l.cache_read_tokens.unwrap_or(0) as i64;
            let cache_write = l.cache_creation_tokens.unwrap_or(0) as i64;
            let total = input + output + cache_read + cache_write;
            SessionInfo {
                id: l.session_id.clone(),
                tool: l.agent.clone(),
                model: l.model.clone(),
                input,
                output,
                cache_read,
                cache_write,
                total,
            }
        })
        .collect()
}

// ------------------------------------------------------------------
// SHA resolution
// ------------------------------------------------------------------

fn resolve_sha(project_root: &str, prefix: &str) -> Vec<(String, String)> {
    let hashes = crate::git::orphan::list_anchor_hashes(project_root);
    hashes
        .into_iter()
        .filter(|h| h.starts_with(prefix))
        .filter_map(|h| {
            let anchor = crate::git::orphan::read_anchor(project_root, &h)?;
            Some((h, anchor.message))
        })
        .collect()
}

// ------------------------------------------------------------------
// emitters  --  list
// ------------------------------------------------------------------

fn emit_list_agent(rows: &[FeedRow], project_name: &str, in_repo: bool) {
    for r in rows {
        let id = short_id(&r.id);
        let rel = relative_time(r.timestamp);
        let subject = truncate_fixed(&r.subject, 40);
        let tool_base = r.tool.as_deref().unwrap_or("-");
        let tool = match &r.worktree_hint {
            Some(wt) => format!("{tool_base}@{}", truncate_fixed(wt, 12)),
            None => tool_base.to_string(),
        };
        let tokens = if r.tokens > 0 {
            human_tokens(r.tokens)
        } else {
            "-".to_string()
        };
        let count = match r.kind {
            RowKind::Anchor if r.ai_pct.is_some() => format!("{}%ai", r.ai_pct.unwrap()),
            RowKind::Shadow => {
                let tid: String = r.id.chars().take(8).collect();
                format!("t:{tid}")
            }
            RowKind::Anchor => "-".to_string(),
        };
        let kind = r.kind.agent_label();
        if in_repo {
            println!("{kind:<6} {id:<10} {rel:<4} {subject:<40} {tool:<20} {tokens:<4} {count}",);
        } else {
            let proj = truncate_fixed(project_name, 14);
            println!("{proj:<14} {kind:<6} {id:<10} {rel:<4} {subject:<40} {tool:<20} {tokens:<4} {count}",);
        }
    }
    if !rows.is_empty() {
        println!();
        println!("commands:");
        println!("  oobo anchor show <sha>   # details for any anchor above");
        println!("  oobo blame <file> [sha]  # per-line AI/human attribution");
        println!("  oobo recall \"query\"      # search sessions");
    }
}

fn emit_list_pretty(rows: &[FeedRow], project_name: &str, in_repo: bool) {
    if rows.is_empty() {
        println!("No anchors yet. Commit through oobo to start anchoring sessions.");
        return;
    }
    println!("\x1b[1moobo memory\x1b[0m");
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
            RowKind::Shadow => {
                let tid: String = r.id.chars().take(8).collect();
                let parent = r
                    .parent_anchor
                    .as_deref()
                    .map(|sha| format!(" · anchored under {}", short_sha(sha)))
                    .unwrap_or_default();
                let wt = r
                    .worktree_hint
                    .as_deref()
                    .map(|w| format!(" · \x1b[36mwt:{w}\x1b[0m"))
                    .unwrap_or_default();
                (
                    "\x1b[2m○\x1b[0m",
                    "\x1b[2manchor\x1b[0m",
                    "\x1b[2m",
                    format!(
                        "{tool} · t:{tid} · {} file{} · {} tool{}{parent}{wt}",
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
            let proj = truncate_fixed(project_name, 16);
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

fn emit_list_json(_cfg: &Config, rows: &[FeedRow], project_name: &str, in_repo: bool) {
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
                obj["project"] = serde_json::Value::String(project_name.to_string());
            }
            match r.kind {
                RowKind::Anchor => {
                    obj["sha"] = serde_json::Value::String(r.id.clone());
                }
                RowKind::Shadow => {
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
                    if let Some(wt) = &r.worktree_hint {
                        obj["worktree"] = serde_json::Value::String(wt.clone());
                    }
                }
            }
            obj
        })
        .collect();
    let json = serde_json::json!({
        "anchors": arr,
        "actions": [
            { "command": "oobo anchor show <sha>", "description": "show anchor details" },
            { "command": "oobo blame <file> [sha]", "description": "line attribution" },
            { "command": "oobo recall \"query\"", "description": "search sessions" },
        ],
    });
    crate::utils::print_json(&json);
}

// ------------------------------------------------------------------
// emitters  --  show
// ------------------------------------------------------------------

fn emit_show_agent(anchor: &Anchor, sessions: &[SessionInfo], v2_refs: &[V2SessionRefInfo]) {
    let ts = chrono::DateTime::from_timestamp(anchor.committed_at, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let tool = sessions.first().map_or("-", |s| s.tool.as_str());
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
        println!("ai_pct:     {p:.0}");
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
    if !v2_refs.is_empty() {
        println!("provenance:");
        for r in v2_refs {
            let uid8 = &r.session_uid[..8.min(r.session_uid.len())];
            let home = r.home_location.as_deref().map_or_else(
                || "home:here".to_string(),
                |h| {
                    use crate::git::orphan::v2::resolve::Hydration;
                    match &r.hydration {
                        Hydration::StubOnly => format!("home:{h} (stub — no access)"),
                        Hydration::Cached { .. } => format!("home:{h} (cached)"),
                        _ => format!("home:{h}"),
                    }
                },
            );
            println!("  s:{uid8} {}t {home}", r.turns_claimed);
        }
    }
    println!();
    println!("commands:");
    if !anchor.files_changed.is_empty() {
        let short = &anchor.commit_hash[..7.min(anchor.commit_hash.len())];
        println!("  oobo blame {} {}", anchor.files_changed[0], short);
    }
    println!("  oobo goto <turn-or-commit>   # travel to this point");
}

fn emit_show_json(
    cfg: &Config,
    anchor: &Anchor,
    sessions: &[SessionInfo],
    v2_refs: &[V2SessionRefInfo],
) {
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
    let first_file = anchor.files_changed.first().cloned().unwrap_or_default();
    let short_sha = &anchor.commit_hash[..7.min(anchor.commit_hash.len())];
    let mut actions: Vec<serde_json::Value> = Vec::new();
    if !first_file.is_empty() {
        actions.push(serde_json::json!({
            "command": format!("oobo blame {} {}", first_file, short_sha),
            "description": "line attribution",
        }));
    }
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
        "sessions_v2": v2_refs,
        "shadow_anchors": anchor.turns,
        "files_changed": anchor.files_changed,
        "actions": actions,
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
        .map(std::string::ToString::to_string)
        .collect()
}

// ------------------------------------------------------------------
// backward-compat shim used by `commands/bare.rs`
// ------------------------------------------------------------------

/// Kept for bare `oobo` in-repo agent/json modes (byte-for-byte equivalence).
pub fn run(cfg: &Config, limit: usize, mode: OutputMode) -> Result<(), CliError> {
    let opts = Options {
        limit,
        ..Default::default()
    };
    run_list(cfg, &opts, mode)?;
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
