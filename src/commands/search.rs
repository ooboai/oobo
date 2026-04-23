//! `oobo search` — local-first search across sessions, anchors, and commit
//! subjects. Remote search is stubbed for v1 (requires API key) and returns
//! an actionable error when invoked.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::db::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Local,
    Remote,
    Both,
}

/// Search scope — which projects to consider.
#[derive(Debug, Clone)]
pub enum Scope {
    /// All projects.
    Global,
    /// A single project referenced by name (case-insensitive).
    Project(String),
    /// The project rooted at the given path (usually the current repo).
    CurrentRepo(String),
}

#[derive(Debug, Clone)]
pub struct Options {
    pub source: Option<Source>,
    pub since: Option<String>,
    pub scope: Scope,
    pub tool: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub project_id: String,
    pub project_name: String,
    pub anchor_sha: Option<String>,
    pub session_id: Option<String>,
    pub tool: Option<String>,
    pub tokens: Option<i64>,
    pub timestamp: Option<i64>,
    pub intent: String,
    pub snippet: String,
    pub score: f64,
}

pub fn run(
    cfg: &Config,
    query: &str,
    opts: Options,
    mode: OutputMode,
) -> Result<i32, String> {
    if query.trim().is_empty() {
        eprintln!("error: query cannot be empty");
        return Ok(2);
    }

    let source = resolve_source(cfg, opts.source)?;
    match source {
        Source::Remote => {
            if cfg.server.api_key.is_empty() {
                eprintln!(
                    "error: --remote requires an API key. run: oobo settings set key <...>"
                );
                return Ok(2);
            }
            // Remote backend not yet wired up in v1.
            eprintln!("error: remote search is not available yet in this build.");
            return Ok(1);
        }
        Source::Both => {
            if cfg.server.api_key.is_empty() {
                eprintln!(
                    "error: --both requires an API key. run: oobo settings set key <...>"
                );
                return Ok(2);
            }
        }
        Source::Local => {}
    }

    let db = Db::open()?;
    let local_hits = search_local(&db, query, &opts)?;

    let mut hits = local_hits;
    let mut sources: Vec<&'static str> = vec!["local"];

    if matches!(source, Source::Both) {
        // Best-effort remote — failure degrades gracefully.
        match search_remote(cfg, query, &opts) {
            Ok(mut remote_hits) => {
                sources.push("remote");
                hits.append(&mut remote_hits);
            }
            Err(e) => {
                eprintln!(
                    "warning: remote search failed: {e}. showing local results only."
                );
            }
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if opts.limit > 0 && hits.len() > opts.limit {
        hits.truncate(opts.limit);
    }
    if opts.limit == 0 {
        hits.clear();
    }

    emit(&hits, query, &sources, mode);
    Ok(0)
}

fn resolve_source(cfg: &Config, explicit: Option<Source>) -> Result<Source, String> {
    if let Some(s) = explicit {
        return Ok(s);
    }
    if cfg.server.api_key.is_empty() {
        Ok(Source::Local)
    } else {
        Ok(Source::Both)
    }
}

/// Public entry used by the TUI's in-app search. Opens the DB, runs the
/// same ranking as `oobo search` (local only), and returns raw hits.
pub fn collect_local(_cfg: &Config, query: &str, opts: &Options) -> Result<Vec<Hit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let db = Db::open()?;
    let mut hits = search_local(&db, query, opts)?;
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if opts.limit > 0 && hits.len() > opts.limit {
        hits.truncate(opts.limit);
    }
    Ok(hits)
}

/// Local search: join sessions ⨝ projects and anchors ⨝ projects, filter by
/// query substring across a few columns, rank by recency + term density.
fn search_local(db: &Db, query: &str, opts: &Options) -> Result<Vec<Hit>, String> {
    let conn = &db.conn;
    let q_terms: Vec<String> = tokenize(query);
    if q_terms.is_empty() {
        return Ok(vec![]);
    }

    // since filter
    let since_ts = opts.since.as_deref().and_then(parse_since);

    // Resolve scope into a project-matching predicate.
    let scope_match: Box<dyn Fn(&str, &str) -> bool> = match &opts.scope {
        Scope::Global => Box::new(|_pid: &str, _pname: &str| true),
        Scope::Project(name) => {
            let name = name.to_string();
            Box::new(move |_pid: &str, pname: &str| pname.eq_ignore_ascii_case(&name))
        }
        Scope::CurrentRepo(root) => {
            // Resolve root → the same stable project id the rest of the DB uses.
            let wanted = crate::project::id_for_root(root);
            Box::new(move |pid: &str, _pname: &str| pid == wanted)
        }
    };

    let mut hits: Vec<Hit> = Vec::new();

    // ── anchors ─────────────────────────────────────────────────────────
    let mut stmt = conn
        .prepare(
            "SELECT a.commit_hash, a.intent, a.message, a.committed_at,
                    p.id, p.name
             FROM anchors a
             JOIN ai_commits c ON c.commit_hash = a.commit_hash
             JOIN projects p   ON p.id = c.project_id
             ORDER BY a.committed_at DESC
             LIMIT 5000",
        )
        .map_err(|e| format!("prepare anchors query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|e| format!("anchors query: {e}"))?;
    for r in rows.flatten() {
        let (sha, intent, message, committed_at, pid, pname) = r;
        let haystack = format!(
            "{} {}",
            intent.clone().unwrap_or_default(),
            message.clone().unwrap_or_default()
        );
        let score = term_score(&haystack, &q_terms);
        if score <= 0.0 {
            continue;
        }
        if let Some(ts) = since_ts {
            if committed_at.unwrap_or(0) < ts {
                continue;
            }
        }
        let pid_s = pid.unwrap_or_default();
        let pname_s = pname.unwrap_or_else(|| "unknown".to_string());
        if !scope_match(&pid_s, &pname_s) {
            continue;
        }
        hits.push(Hit {
            project_id: pid_s,
            project_name: pname_s,
            anchor_sha: Some(short_sha(&sha)),
            session_id: None,
            tool: None,
            tokens: None,
            timestamp: committed_at,
            intent: intent.unwrap_or_default(),
            snippet: snippet(&haystack, &q_terms),
            score,
        });
    }
    drop(stmt);

    // ── sessions ────────────────────────────────────────────────────────
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.source, s.first_message, s.updated_at,
                    p.id, p.name
             FROM sessions s
             JOIN projects p ON p.id = s.project_id
             ORDER BY s.updated_at DESC
             LIMIT 5000",
        )
        .map_err(|e| format!("prepare sessions query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| format!("sessions query: {e}"))?;
    for r in rows.flatten() {
        let (sid, src, first_message, updated_at, pid, pname) = r;
        if !scope_match(&pid, &pname) {
            continue;
        }
        if let Some(tf) = opts.tool.as_deref() {
            if !src.eq_ignore_ascii_case(tf) {
                continue;
            }
        }
        let haystack = first_message.clone().unwrap_or_default();
        let score = term_score(&haystack, &q_terms);
        if score <= 0.0 {
            continue;
        }
        if let Some(ts) = since_ts {
            if updated_at.unwrap_or(0) < ts {
                continue;
            }
        }
        hits.push(Hit {
            project_id: pid,
            project_name: pname,
            anchor_sha: None,
            session_id: Some(sid),
            tool: Some(src),
            tokens: None,
            timestamp: updated_at,
            intent: haystack.chars().take(60).collect(),
            snippet: snippet(&haystack, &q_terms),
            score,
        });
    }

    Ok(hits)
}

fn search_remote(
    _cfg: &Config,
    _query: &str,
    _opts: &Options,
) -> Result<Vec<Hit>, String> {
    Err("remote backend not implemented in this build".to_string())
}

fn tokenize(q: &str) -> Vec<String> {
    q.split_whitespace()
        .map(|s| s.trim_matches('"').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn term_score(haystack: &str, terms: &[String]) -> f64 {
    let hay = haystack.to_ascii_lowercase();
    let mut hits = 0;
    for t in terms {
        if hay.contains(t) {
            hits += 1;
        }
    }
    if hits == 0 {
        return 0.0;
    }
    hits as f64 / terms.len() as f64
}

fn snippet(text: &str, terms: &[String]) -> String {
    let text_lc = text.to_ascii_lowercase();
    let idx = terms
        .iter()
        .filter_map(|t| text_lc.find(t))
        .min()
        .unwrap_or(0);
    let start = idx.saturating_sub(20);
    let end = (idx + 80).min(text.len());
    // Walk to a char boundary.
    let mut s = start;
    while !text.is_char_boundary(s) && s > 0 {
        s -= 1;
    }
    let mut e = end;
    while !text.is_char_boundary(e) && e < text.len() {
        e += 1;
    }
    let mut out = text[s..e].to_string();
    if e < text.len() {
        out.push_str("...");
    }
    out
}

fn parse_since(s: &str) -> Option<i64> {
    let now = chrono::Utc::now().timestamp();
    if let Some(stripped) = s.strip_suffix('d') {
        return stripped.parse::<i64>().ok().map(|d| now - d * 86400);
    }
    if let Some(stripped) = s.strip_suffix('h') {
        return stripped.parse::<i64>().ok().map(|h| now - h * 3600);
    }
    if let Some(stripped) = s.strip_suffix('m') {
        return stripped.parse::<i64>().ok().map(|m| now - m * 60);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn relative_time(ts: i64) -> String {
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

fn human_tokens(tokens: i64) -> String {
    if tokens >= 1000 {
        format!("{}k", tokens / 1000)
    } else {
        tokens.to_string()
    }
}

// ── emitters ───────────────────────────────────────────────────────────────

fn emit(hits: &[Hit], query: &str, sources: &[&str], mode: OutputMode) {
    match mode {
        OutputMode::Json => emit_json(hits, query, sources),
        OutputMode::Agent => emit_agent(hits),
        OutputMode::Tui => emit_pretty(hits, query),
    }
}

fn emit_agent(hits: &[Hit]) {
    let multi_project = hits
        .iter()
        .map(|h| &h.project_name)
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 1;
    for h in hits {
        let sha = h.anchor_sha.clone().unwrap_or_else(|| "-".to_string());
        let tool = h.tool.clone().unwrap_or_else(|| "-".to_string());
        let tokens = h.tokens.map(human_tokens).unwrap_or_else(|| "-".to_string());
        let when = h.timestamp.map(relative_time).unwrap_or_else(|| "-".to_string());
        let snippet: String = h.snippet.chars().take(60).collect();
        if multi_project {
            println!(
                "{:<10} {} {} {} {} {}",
                h.project_name, sha, tool, tokens, when, snippet
            );
        } else {
            println!("{sha} {tool} {tokens} {when} {snippet}");
        }
    }
}

fn emit_pretty(hits: &[Hit], query: &str) {
    if hits.is_empty() {
        println!("no results for \"{query}\"");
        return;
    }
    for h in hits {
        let tool = h.tool.clone().unwrap_or_else(|| "-".to_string());
        let when = h.timestamp.map(relative_time).unwrap_or_else(|| "-".to_string());
        let intent = if h.intent.is_empty() {
            "(no intent)"
        } else {
            &h.intent
        };
        println!("\x1b[1m{}\x1b[0m · {tool} · {when}     {intent}", h.project_name);
        println!("  \"{}\"", h.snippet);
        if let Some(sha) = &h.anchor_sha {
            let tokens = h
                .tokens
                .map(|t| format!(" · {} tokens", human_tokens(t)))
                .unwrap_or_default();
            println!("  anchor {sha}{tokens}");
        }
        println!();
    }
}

fn emit_json(hits: &[Hit], query: &str, sources: &[&str]) {
    let arr: Vec<serde_json::Value> = hits
        .iter()
        .map(|h| {
            serde_json::json!({
                "project": { "id": h.project_id, "name": h.project_name },
                "anchor_sha": h.anchor_sha,
                "session_id": h.session_id,
                "tool": h.tool,
                "tokens": h.tokens,
                "timestamp": h.timestamp,
                "intent": h.intent,
                "snippet": h.snippet,
                "score": h.score,
            })
        })
        .collect();
    let json = serde_json::json!({
        "query": query,
        "sources": sources,
        "total_hits": hits.len(),
        "hits": arr,
    });
    crate::utils::print_json(&json);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize() {
        assert_eq!(tokenize("hello world"), vec!["hello", "world"]);
        assert_eq!(tokenize("  "), Vec::<String>::new());
    }

    #[test]
    fn test_term_score() {
        assert_eq!(term_score("the auth middleware", &["auth".to_string()]), 1.0);
        assert_eq!(
            term_score("the auth middleware", &["auth".to_string(), "foo".to_string()]),
            0.5
        );
        assert_eq!(term_score("nothing", &["auth".to_string()]), 0.0);
    }

    #[test]
    fn test_parse_since() {
        assert!(parse_since("7d").is_some());
        assert!(parse_since("24h").is_some());
        assert!(parse_since("30m").is_some());
        assert!(parse_since("bogus").is_none());
    }

    #[test]
    fn test_short_sha() {
        assert_eq!(short_sha("abcdef1234567890"), "abcdef1");
    }

    #[test]
    fn test_human_tokens() {
        assert_eq!(human_tokens(500), "500");
        assert_eq!(human_tokens(12345), "12k");
    }
}
