//! `oobo anchors` — the flagship view.
//!
//! List anchors (in or out of a repo) and drill into a single anchor via
//! `anchors show <sha>`. See `tests/cli-spec/02-anchors.md` for the exact
//! output contract.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::anchor::Anchor;
use crate::db::Db;

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
    let db = Db::open()?;

    let in_repo = crate::git::proxy::project_root(cfg).is_some();

    if in_repo && opts.project.is_some() {
        eprintln!(
            "error: --project is not allowed inside a repo (current project is implied)"
        );
        return Ok(2);
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
        load_in_repo(cfg, &db, &opts, since_epoch)?
    } else {
        load_cross_project(&db, &opts, since_epoch)?
    };

    // Disabled-project guard (in-repo only).
    if in_repo {
        if let Some(root) = crate::git::proxy::project_root(cfg) {
            if let Ok(pid) = project_id_from_root(&db, &root) {
                let s = db.get_project_settings(&pid).unwrap_or_default();
                if s.ignored {
                    match mode {
                        OutputMode::Tui => {
                            println!("oobo is disabled for this project. run: oobo enable");
                        }
                        OutputMode::Agent => println!("disabled"),
                        OutputMode::Json => {
                            crate::utils::print_json(&serde_json::Value::Array(vec![]));
                        }
                    }
                    return Ok(0);
                }
            }
        }
    }

    match mode {
        OutputMode::Json => emit_list_json(cfg, &db, &rows, in_repo),
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
    let db = Db::open()?;

    let matches = resolve_sha(&db, cfg, sha)?;
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
    let anchor = load_anchor(&db, &commit_hash)
        .ok_or_else(|| format!("anchor row missing for {commit_hash}"))?;

    let sessions = load_sessions(&db, &commit_hash);

    match mode {
        OutputMode::Json => emit_show_json(&anchor, &sessions),
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
    project_name: String,
    sha: String,
    subject: String,
    timestamp: i64,
    tool: Option<String>,
    tokens: i64,
    session_count: usize,
    ai_pct: Option<i64>,
}

// ------------------------------------------------------------------
// loaders
// ------------------------------------------------------------------

fn load_in_repo(
    cfg: &Config,
    db: &Db,
    opts: &Options,
    since: Option<i64>,
) -> Result<Vec<Row>, String> {
    // Walk recent git log, enrich from DB.
    let n = opts.limit.max(1);
    let log = crate::git::proxy::run_git_capture(
        cfg,
        &[
            "log",
            &format!("-{}", n * 4), // over-fetch; filters may drop rows
            "--format=%H|||%s|||%ct",
        ],
    )
    .unwrap_or_default();

    let project_name = crate::git::proxy::project_root(cfg)
        .and_then(|p| {
            std::path::Path::new(&p)
                .file_name()
                .and_then(|s| s.to_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| "-".to_string());

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

        let (tool, tokens, count) = load_session_summary(db, &sha);

        if let Some(t) = opts.tool.as_deref() {
            let want = t.to_lowercase();
            let has = tool.as_deref().map(|x| x.to_lowercase()).unwrap_or_default();
            if has != want {
                continue;
            }
        }

        let ai_pct = load_ai_pct(db, &sha);

        out.push(Row {
            project_name: project_name.clone(),
            sha,
            subject,
            timestamp: ts,
            tool,
            tokens,
            session_count: count,
            ai_pct,
        });
        if out.len() >= opts.limit {
            break;
        }
    }
    Ok(out)
}

fn load_cross_project(db: &Db, opts: &Options, since: Option<i64>) -> Result<Vec<Row>, String> {
    use rusqlite::params;

    let mut sql = String::from(
        "SELECT p.name, a.commit_hash, a.message, a.committed_at
         FROM anchors a
         JOIN ai_commits c ON c.commit_hash = a.commit_hash
         JOIN projects p   ON p.id          = c.project_id",
    );
    let mut where_: Vec<String> = Vec::new();
    let mut args: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(proj) = opts.project.as_deref() {
        where_.push(format!(
            "(p.name = ?{n} OR p.path = ?{n})",
            n = args.len() + 1
        ));
        args.push(rusqlite::types::Value::Text(proj.to_string()));
    }
    if let Some(s) = since {
        where_.push(format!("a.committed_at >= ?{n}", n = args.len() + 1));
        args.push(s.into());
    }
    if !where_.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_.join(" AND "));
    }
    sql.push_str(" ORDER BY a.committed_at DESC LIMIT ?");
    args.push((opts.limit.max(1) as i64).into());

    let mut stmt = db
        .conn
        .prepare(&sql)
        .map_err(|e| format!("prepare anchors: {e}"))?;
    let rows_iter = stmt
        .query_map(params_from_iter(&args), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(|e| format!("query anchors: {e}"))?;

    let mut out: Vec<Row> = Vec::new();
    for r in rows_iter.flatten() {
        let (project_name, sha, msg, ts) = r;
        let (tool, tokens, count) = load_session_summary(db, &sha);
        if let Some(t) = opts.tool.as_deref() {
            let want = t.to_lowercase();
            let has = tool.as_deref().map(|x| x.to_lowercase()).unwrap_or_default();
            if has != want {
                continue;
            }
        }
        out.push(Row {
            project_name,
            sha,
            subject: msg.unwrap_or_default(),
            timestamp: ts.unwrap_or(0),
            tool,
            tokens,
            session_count: count,
            ai_pct: None,
        });
    }
    Ok(out)
}

/// Primary tool + total tokens + session count for a commit.
fn load_session_summary(db: &Db, commit_hash: &str) -> (Option<String>, i64, usize) {
    let mut stmt = match db.conn.prepare(
        "SELECT agent,
                COALESCE(input_tokens,0)+COALESCE(output_tokens,0)+
                COALESCE(cache_read_tokens,0)+COALESCE(cache_creation_tokens,0) AS t
         FROM anchor_sessions WHERE commit_hash = ?1",
    ) {
        Ok(s) => s,
        Err(_) => return (None, 0, 0),
    };
    let Ok(rows) = stmt.query_map([commit_hash], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    }) else {
        return (None, 0, 0);
    };

    let mut total: i64 = 0;
    let mut first_tool: Option<String> = None;
    let mut count: usize = 0;
    for r in rows.flatten() {
        if first_tool.is_none() {
            first_tool = Some(r.0);
        }
        total += r.1;
        count += 1;
    }
    (first_tool, total, count)
}

fn load_ai_pct(db: &Db, commit_hash: &str) -> Option<i64> {
    db.conn
        .query_row(
            "SELECT ai_percentage FROM ai_commits WHERE commit_hash = ?1",
            [commit_hash],
            |r| r.get::<_, Option<f64>>(0),
        )
        .ok()
        .flatten()
        .map(|p| p.round() as i64)
}

fn load_anchor(db: &Db, commit_hash: &str) -> Option<Anchor> {
    let raw: String = db
        .conn
        .query_row(
            "SELECT raw_json FROM anchors WHERE commit_hash = ?1",
            [commit_hash],
            |row| row.get(0),
        )
        .ok()?;
    serde_json::from_str(&raw).ok()
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

fn load_sessions(db: &Db, commit_hash: &str) -> Vec<SessionInfo> {
    let mut stmt = match db.conn.prepare(
        "SELECT session_id, agent, model,
                COALESCE(input_tokens,0),
                COALESCE(output_tokens,0),
                COALESCE(cache_read_tokens,0),
                COALESCE(cache_creation_tokens,0)
         FROM anchor_sessions WHERE commit_hash = ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt
        .query_map([commit_hash], |r| {
            Ok(SessionInfo {
                id: r.get(0)?,
                tool: r.get(1)?,
                model: r.get(2)?,
                input: r.get(3)?,
                output: r.get(4)?,
                cache_read: r.get(5)?,
                cache_write: r.get(6)?,
                total: 0,
            })
        })
        .ok();
    let mut out: Vec<SessionInfo> = Vec::new();
    if let Some(rs) = rows {
        for s in rs.flatten() {
            let total = s.input + s.output + s.cache_read + s.cache_write;
            out.push(SessionInfo { total, ..s });
        }
    }
    out
}

// ------------------------------------------------------------------
// SHA resolution
// ------------------------------------------------------------------

fn resolve_sha(db: &Db, _cfg: &Config, prefix: &str) -> Result<Vec<(String, String)>, String> {
    let like = format!("{prefix}%");
    let mut stmt = db
        .conn
        .prepare("SELECT commit_hash, COALESCE(message,'') FROM anchors WHERE commit_hash LIKE ?1 LIMIT 8")
        .map_err(|e| format!("prepare resolve: {e}"))?;
    let rows = stmt
        .query_map([&like], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("query resolve: {e}"))?;
    let mut out: Vec<(String, String)> = Vec::new();
    for r in rows.flatten() {
        out.push(r);
    }
    Ok(out)
}

fn project_id_from_root(db: &Db, root: &str) -> Result<String, String> {
    db.conn
        .query_row(
            "SELECT id FROM projects WHERE path = ?1 LIMIT 1",
            [root],
            |r| r.get::<_, String>(0),
        )
        .map_err(|e| format!("project not found: {e}"))
}

// ------------------------------------------------------------------
// emitters — list
// ------------------------------------------------------------------

fn emit_list_agent(rows: &[Row], in_repo: bool) {
    for r in rows {
        let sha7 = short_sha(&r.sha);
        let rel = relative_time(r.timestamp);
        let subject = truncate_fixed(&r.subject, 40);
        let tool = r.tool.as_deref().unwrap_or("-");
        let tokens = if r.tokens > 0 {
            human_tokens(r.tokens)
        } else {
            "-".to_string()
        };
        let count = if r.session_count > 0 {
            format!("{}s", r.session_count)
        } else {
            "-".to_string()
        };
        if in_repo {
            println!(
                "{sha7} {rel:<4} {subject:<40} {tool:<7} {tokens:<4} {count}",
            );
        } else {
            let proj = truncate_fixed(&r.project_name, 14);
            println!(
                "{proj:<14} {sha7} {rel:<4} {subject:<40} {tool:<7} {tokens:<4} {count}",
            );
        }
    }
}

fn emit_list_pretty(rows: &[Row], in_repo: bool) {
    if rows.is_empty() {
        println!("No anchors yet. Commit through oobo to start anchoring sessions.");
        return;
    }
    for r in rows {
        let sha7 = short_sha(&r.sha);
        let rel = relative_time(r.timestamp);
        let subject = truncate_fixed(&r.subject, 40);
        let tool = r.tool.as_deref().unwrap_or("-");
        let tokens = if r.tokens > 0 {
            human_tokens(r.tokens)
        } else {
            "-".to_string()
        };
        let sessions = match r.session_count {
            0 => "(local only)".to_string(),
            1 => "1 session".to_string(),
            n => format!("{n} sessions"),
        };
        let ai_pct = r
            .ai_pct
            .map(|p| format!(" · \x1b[35m{p}% AI\x1b[0m"))
            .unwrap_or_default();
        if in_repo {
            println!(
                "  \x1b[33m{sha7}\x1b[0m  \x1b[2m{rel:<4}\x1b[0m  {subject:<40}  \x1b[36m{tool}\x1b[0m · {tokens} · {sessions}{ai_pct}",
            );
        } else {
            let proj = truncate_fixed(&r.project_name, 16);
            println!(
                "  \x1b[34m{proj:<16}\x1b[0m  \x1b[33m{sha7}\x1b[0m  \x1b[2m{rel:<4}\x1b[0m  {subject:<40}  \x1b[36m{tool}\x1b[0m · {tokens} · {sessions}{ai_pct}",
            );
        }
    }
}

fn emit_list_json(_cfg: &Config, _db: &Db, rows: &[Row], in_repo: bool) {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut obj = serde_json::json!({
                "sha": r.sha,
                "timestamp": chrono::DateTime::from_timestamp(r.timestamp, 0)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default(),
                "subject": r.subject,
                "tools": r.tool.clone().map(|t| vec![t]).unwrap_or_default(),
                "tokens": { "total": r.tokens },
                "sessions_count": r.session_count,
                "ai_pct": r.ai_pct,
            });
            // `project` is only meaningful in cross-project listings.
            if !in_repo {
                obj["project"] = serde_json::Value::String(r.project_name.clone());
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
        println!("\x1b[2m{author}  ·  {rel} ago\x1b[0m", author = anchor.author);
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

fn emit_show_json(anchor: &Anchor, sessions: &[SessionInfo]) {
    let ts = chrono::DateTime::from_timestamp(anchor.committed_at, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
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
        "parents": Vec::<String>::new(),
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

fn truncate_fixed(s: &str, n: usize) -> String {
    let mut t: String = s.chars().take(n).collect();
    while t.chars().count() < n {
        t.push(' ');
    }
    t
}

fn human_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
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
    } else if d < 7 * 86400 {
        format!("{}d", d / 86400)
    } else if d < 30 * 86400 {
        format!("{}w", d / (7 * 86400))
    } else if d < 365 * 86400 {
        format!("{}mo", d / (30 * 86400))
    } else {
        format!("{}y", d / (365 * 86400))
    }
}

/// Accept durations (`24h`, `7d`, `30m`, `1mo`, `1y`) or ISO-8601 timestamps.
fn parse_since(raw: &str) -> Result<i64, String> {
    if let Ok(dt) = raw.parse::<chrono::DateTime<chrono::Utc>>() {
        return Ok(dt.timestamp());
    }
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err("expected number + suffix (s/m/h/d/w/mo/y) or ISO-8601".into());
    }
    let n: i64 = digits.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let suffix = &raw[digits.len()..];
    let seconds: i64 = match suffix {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        "w" => n * 7 * 86400,
        "mo" => n * 30 * 86400,
        "y" => n * 365 * 86400,
        other => return Err(format!("unknown suffix '{other}'")),
    };
    Ok(chrono::Utc::now().timestamp() - seconds)
}

fn params_from_iter<'a>(
    vals: &'a [rusqlite::types::Value],
) -> impl rusqlite::Params + 'a {
    rusqlite::params_from_iter(vals.iter())
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
    fn test_human_tokens() {
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(1500), "1k");
        assert_eq!(human_tokens(2_500_000), "2.5M");
    }
}
