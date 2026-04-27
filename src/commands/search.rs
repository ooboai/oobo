//! `oobo search` — local-first search across sessions, anchors, and commit
//! subjects. When an API key is configured, remote results are merged in.

use crate::cli::OutputMode;
use crate::config::Config;

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

pub fn run(cfg: &Config, query: &str, opts: Options, mode: OutputMode) -> Result<i32, String> {
    if query.trim().is_empty() {
        eprintln!("error: query cannot be empty");
        return Ok(2);
    }

    let effective_key = crate::commands::sync::resolve_api_key(cfg);
    let source = resolve_source(&effective_key, opts.source)?;
    match source {
        Source::Remote => {
            if effective_key.is_empty() {
                eprintln!("error: --remote requires an API key. run: anchor settings set key <...>");
                return Ok(2);
            }
        }
        Source::Both => {
            if effective_key.is_empty() {
                eprintln!("error: --both requires an API key. run: anchor settings set key <...>");
                return Ok(2);
            }
        }
        Source::Local => {}
    }

    if matches!(source, Source::Remote) {
        match search_remote(cfg, &effective_key, query, &opts) {
            Ok(mut hits) => {
                sort_and_limit(&mut hits, opts.limit);
                emit(&hits, query, &["remote"], mode);
                return Ok(0);
            }
            Err(e) => {
                eprintln!("error: remote search failed: {e}");
                return Ok(1);
            }
        }
    }

    let project_root = crate::git::proxy::project_root(cfg);
    let local_hits = search_local(project_root.as_deref(), query, &opts)?;

    let mut hits = local_hits;
    let mut sources: Vec<&'static str> = vec!["local"];

    if matches!(source, Source::Both) {
        match search_remote(cfg, &effective_key, query, &opts) {
            Ok(mut remote_hits) => {
                sources.push("remote");
                hits.append(&mut remote_hits);
            }
            Err(e) => {
                eprintln!("warning: remote search failed: {e}. showing local results only.");
            }
        }
    }

    sort_and_limit(&mut hits, opts.limit);

    emit(&hits, query, &sources, mode);
    Ok(0)
}

fn resolve_source(api_key: &str, explicit: Option<Source>) -> Result<Source, String> {
    if let Some(s) = explicit {
        return Ok(s);
    }
    if api_key.is_empty() {
        Ok(Source::Local)
    } else {
        Ok(Source::Both)
    }
}

/// Public entry used by the TUI's in-app search. Runs the same ranking as
/// `oobo search` (local only) and returns raw hits.
pub fn collect_local(cfg: &Config, query: &str, opts: &Options) -> Result<Vec<Hit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let project_root = crate::git::proxy::project_root(cfg);
    let mut hits = search_local(project_root.as_deref(), query, opts)?;
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

fn search_local(project_root: Option<&str>, query: &str, opts: &Options) -> Result<Vec<Hit>, String> {
    let q_terms: Vec<String> = tokenize(query);
    if q_terms.is_empty() {
        return Ok(vec![]);
    }
    let since_ts = opts.since.as_deref().and_then(parse_since);
    let mut hits: Vec<Hit> = Vec::new();

    let Some(root) = project_root else {
        return Ok(hits);
    };

    let hashes = crate::git::orphan::list_anchor_hashes(root);
    for hash in &hashes {
        let anchor = match crate::git::orphan::read_anchor(root, hash) {
            Some(a) => a,
            None => continue,
        };
        let haystack = format!("{} {}", anchor.intent.as_deref().unwrap_or(""), anchor.message);
        let score = term_score(&haystack, &q_terms);
        if score <= 0.0 { continue; }
        if let Some(ts) = since_ts {
            if anchor.committed_at < ts { continue; }
        }
        let project_name = std::path::Path::new(root)
            .file_name().and_then(|s| s.to_str())
            .unwrap_or("unknown").to_string();
        hits.push(Hit {
            project_id: crate::project::id_for_root(root),
            project_name,
            anchor_sha: Some(short_sha(hash)),
            session_id: None,
            tool: None,
            tokens: None,
            timestamp: Some(anchor.committed_at),
            intent: anchor.intent.unwrap_or_default(),
            snippet: snippet(&haystack, &q_terms),
            score,
        });
    }
    Ok(hits)
}

fn search_remote(
    cfg: &Config,
    api_key: &str,
    query: &str,
    opts: &Options,
) -> Result<Vec<Hit>, String> {
    let request = crate::remote::payload::SearchRequest {
        query: query.to_string(),
        since: opts.since.clone(),
        project: Some(match &opts.scope {
            Scope::Global => crate::remote::payload::SearchProjectScope {
                kind: "global".to_string(),
                value: None,
            },
            Scope::Project(name) => crate::remote::payload::SearchProjectScope {
                kind: "project_name".to_string(),
                value: Some(name.clone()),
            },
            Scope::CurrentRepo(root) => crate::remote::payload::SearchProjectScope {
                kind: "project_id".to_string(),
                value: Some(crate::project::id_for_root(root)),
            },
        }),
        tool: opts.tool.clone(),
        limit: opts.limit,
    };

    let response = crate::remote::search_anchors_with_timeout(
        cfg,
        &request,
        Some(api_key),
        std::time::Duration::from_secs(5),
    )?;

    Ok(response
        .hits
        .into_iter()
        .map(|h| Hit {
            project_id: h.project.id.unwrap_or_default(),
            project_name: h.project.name.unwrap_or_else(|| "remote".to_string()),
            anchor_sha: h.anchor_sha.map(|sha| short_sha(&sha)),
            session_id: h.session_id,
            tool: h.tool,
            tokens: h.tokens,
            timestamp: h.timestamp,
            intent: h.intent.unwrap_or_default(),
            snippet: h.snippet.unwrap_or_default(),
            score: h.score.unwrap_or(0.0),
        })
        .collect())
}

fn sort_and_limit(hits: &mut Vec<Hit>, limit: usize) {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if limit > 0 && hits.len() > limit {
        hits.truncate(limit);
    }
    if limit == 0 {
        hits.clear();
    }
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
    crate::utils::parse_since(s).ok()
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn relative_time(ts: i64) -> String {
    crate::utils::relative_time(ts)
}

fn human_tokens(tokens: i64) -> String {
    crate::utils::human_tokens(tokens)
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
        let tokens = h
            .tokens
            .map(human_tokens)
            .unwrap_or_else(|| "-".to_string());
        let when = h
            .timestamp
            .map(relative_time)
            .unwrap_or_else(|| "-".to_string());
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
        let when = h
            .timestamp
            .map(relative_time)
            .unwrap_or_else(|| "-".to_string());
        let intent = if h.intent.is_empty() {
            "(no intent)"
        } else {
            &h.intent
        };
        println!(
            "\x1b[1m{}\x1b[0m · {tool} · {when}     {intent}",
            h.project_name
        );
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
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn test_tokenize() {
        assert_eq!(tokenize("hello world"), vec!["hello", "world"]);
        assert_eq!(tokenize("  "), Vec::<String>::new());
    }

    #[test]
    fn test_term_score() {
        assert_eq!(
            term_score("the auth middleware", &["auth".to_string()]),
            1.0
        );
        assert_eq!(
            term_score(
                "the auth middleware",
                &["auth".to_string(), "foo".to_string()]
            ),
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

    #[test]
    fn resolve_source_defaults_to_both_when_key_exists() {
        assert_eq!(resolve_source("", None).unwrap(), Source::Local);
        assert_eq!(resolve_source("sk_test", None).unwrap(), Source::Both);
        assert_eq!(
            resolve_source("sk_test", Some(Source::Remote)).unwrap(),
            Source::Remote
        );
    }

    #[test]
    fn remote_search_posts_contract_and_maps_hits() {
        let body = r#"{
          "hits": [
            {
              "project": { "id": "p1", "name": "oobo-cli" },
              "anchor_sha": "abcdef123456",
              "session_id": "sess-1",
              "tool": "claude",
              "tokens": 12000,
              "timestamp": 1773282899,
              "intent": "fix auth middleware",
              "snippet": "auth middleware token refresh",
              "score": 0.91
            }
          ]
        }"#;
        let (url, requests) = serve_once("200 OK", body);
        let cfg = Config {
            server: crate::config::ServerConfig {
                url,
                api_key: "sk_unused".to_string(),
            },
            ..Config::default()
        };
        let opts = Options {
            source: Some(Source::Remote),
            since: Some("7d".to_string()),
            scope: Scope::Project("oobo-cli".to_string()),
            tool: Some("claude".to_string()),
            limit: 5,
        };

        let hits = search_remote(&cfg, "sk_test", "auth middleware", &opts).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].project_id, "p1");
        assert_eq!(hits[0].project_name, "oobo-cli");
        assert_eq!(hits[0].anchor_sha.as_deref(), Some("abcdef1"));
        assert_eq!(hits[0].session_id.as_deref(), Some("sess-1"));
        assert_eq!(hits[0].tool.as_deref(), Some("claude"));
        assert_eq!(hits[0].tokens, Some(12000));
        assert_eq!(hits[0].score, 0.91);

        let request = requests.recv().unwrap();
        let request_lc = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /anchors/search HTTP/1.1"));
        assert!(request_lc.contains("authorization: bearer sk_test"));
        assert!(request.contains(r#""query":"auth middleware""#));
        assert!(request.contains(r#""since":"7d""#));
        assert!(request.contains(r#""kind":"project_name""#));
        assert!(request.contains(r#""value":"oobo-cli""#));
        assert!(request.contains(r#""tool":"claude""#));
        assert!(request.contains(r#""limit":5"#));
    }

    fn serve_once(status: &str, body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_string();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            tx.send(request).unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        (format!("http://{addr}"), rx)
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).to_string()
    }
}
