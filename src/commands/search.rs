//! `oobo search` — local-first search across sessions, anchors, and commit
//! subjects. When an API key is configured, remote results are merged in.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::{CliError, CmdResult};

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
    /// `"local"`, `"fts"`, or `"memory"`.
    pub source: String,
    pub memory_id: Option<String>,
    pub author: Option<String>,
}

#[tracing::instrument(skip_all, fields(query))]
pub async fn run(cfg: &Config, query: &str, opts: &Options, mode: OutputMode) -> CmdResult {
    if query.trim().is_empty() {
        eprintln!("error: query cannot be empty");
        return Ok(2);
    }

    let project_root = crate::git::proxy::project_root(cfg);
    let resolved = crate::commands::sync::resolve(cfg, project_root.as_deref());
    let source = resolve_source(&resolved.api_key, opts.source);

    if !matches!(source, Source::Remote) && project_root.is_none() {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    }
    match source {
        Source::Remote => {
            if !resolved.has_api_key() {
                eprintln!("error: --remote requires an API key. run: oobo settings set key <...>");
                return Ok(2);
            }
        }
        Source::Both => {
            if !resolved.has_api_key() {
                eprintln!("error: --both requires an API key. run: oobo settings set key <...>");
                return Ok(2);
            }
        }
        Source::Local => {}
    }

    if matches!(source, Source::Remote) {
        match search_remote(&resolved.api_key, &resolved.api_url, query, opts).await {
            Ok(RemoteResult { mut hits, answer }) => {
                sort_and_limit(&mut hits, opts.limit);
                emit(&hits, query, &["remote"], true, answer.as_deref(), mode);
                return Ok(0);
            }
            Err(e) => {
                eprintln!("error: remote search failed: {e}");
                return Ok(1);
            }
        }
    }

    let local_hits = search_local(project_root.as_deref(), query, opts);

    let mut hits = local_hits;
    let mut sources: Vec<&'static str> = vec!["local"];
    let mut answer: Option<String> = None;

    if matches!(source, Source::Both) {
        match search_remote(&resolved.api_key, &resolved.api_url, query, opts).await {
            Ok(remote) => {
                sources.push("remote");
                answer = remote.answer;
                hits.extend(remote.hits);
            }
            Err(e) => {
                tracing::debug!("remote search failed: {e}");
                sources.push("remote_failed");
            }
        }
    }

    sort_and_limit(&mut hits, opts.limit);

    emit(&hits, query, &sources, resolved.has_api_key(), answer.as_deref(), mode);
    Ok(0)
}

fn resolve_source(api_key: &str, explicit: Option<Source>) -> Source {
    if let Some(s) = explicit {
        return s;
    }
    if api_key.is_empty() {
        Source::Local
    } else {
        Source::Both
    }
}

/// Public entry used by the TUI's in-app search. Runs the same ranking as
/// `oobo search` (local only) and returns raw hits.
pub fn collect_local(cfg: &Config, query: &str, opts: &Options) -> Result<Vec<Hit>, CliError> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let project_root = crate::git::proxy::project_root(cfg);
    let mut hits = search_local(project_root.as_deref(), query, opts);
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

fn search_local(project_root: Option<&str>, query: &str, opts: &Options) -> Vec<Hit> {
    let q_terms: Vec<String> = tokenize(query);
    if q_terms.is_empty() {
        return vec![];
    }
    let since_ts = opts.since.as_deref().and_then(parse_since);
    let mut hits: Vec<Hit> = Vec::new();

    let Some(root) = project_root else {
        return hits;
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
            source: "local".to_string(),
            memory_id: None,
            author: None,
        });
    }
    hits
}

struct RemoteResult {
    answer: Option<String>,
    hits: Vec<Hit>,
}

async fn search_remote(
    api_key: &str,
    base_url: &str,
    query: &str,
    opts: &Options,
) -> Result<RemoteResult, CliError> {
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
        &request,
        api_key,
        base_url,
        std::time::Duration::from_secs(15),
    )
    .await?;

    let hits = response
        .hits
        .into_iter()
        .map(|h| {
            let session_id = h.session_id.or_else(|| {
                h.session_ids.as_ref().and_then(|ids| ids.first().cloned())
            });
            Hit {
                project_id: h.project.id.unwrap_or_default(),
                project_name: h.project.name.unwrap_or_else(|| "remote".to_string()),
                anchor_sha: h.anchor_sha.map(|sha| short_sha(&sha)),
                session_id,
                tool: h.tool,
                tokens: h.tokens,
                timestamp: h.timestamp,
                intent: h.intent.unwrap_or_default(),
                snippet: h.snippet.unwrap_or_default(),
                score: h.score.unwrap_or(0.0),
                source: h.source.unwrap_or_else(|| "fts".to_string()),
                memory_id: h.memory_id,
                author: h.author,
            }
        })
        .collect();

    Ok(RemoteResult {
        answer: response.answer,
        hits,
    })
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
    f64::from(hits) / terms.len() as f64
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

fn emit(hits: &[Hit], query: &str, sources: &[&str], has_key: bool, answer: Option<&str>, mode: OutputMode) {
    match mode {
        OutputMode::Json => emit_json(hits, query, sources, answer),
        OutputMode::Agent => emit_agent(hits, sources, has_key, answer),
        OutputMode::Tui => emit_pretty(hits, query, sources, has_key, answer),
    }
}

fn emit_agent(hits: &[Hit], _sources: &[&str], has_key: bool, answer: Option<&str>) {
    if let Some(ans) = answer {
        println!("answer: {ans}");
        println!();
    }
    if hits.is_empty() && answer.is_none() {
        println!("no results");
        if !has_key {
            println!("note: cloud search not configured. run: oobo settings set key <API_KEY>");
        }
        return;
    }
    if hits.is_empty() {
        return;
    }
    let multi_project = hits
        .iter()
        .map(|h| &h.project_name)
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 1;
    for h in hits {
        let is_memory = h.source == "memory";
        let tag = if is_memory { "[memory] " } else { "" };
        let sha = h.anchor_sha.clone().unwrap_or_else(|| "-".to_string());
        let tool = h.tool.clone().unwrap_or_else(|| "-".to_string());
        let tokens = h
            .tokens.map_or_else(|| "-".to_string(), human_tokens);
        let when = h
            .timestamp.map_or_else(|| "-".to_string(), relative_time);
        let snippet: String = h.snippet.chars().take(60).collect();
        if multi_project {
            println!(
                "{tag}{:<10} {} {} {} {} {}",
                h.project_name, sha, tool, tokens, when, snippet
            );
        } else {
            println!("{tag}{sha} {tool} {tokens} {when} {snippet}");
        }
    }
    if !hits.is_empty() {
        println!();
        println!("commands:");
        println!("  oobo anchor show <sha>       # details for any result above");
        println!("  oobo search \"query\" --json   # structured output");
    }
}

fn emit_pretty(hits: &[Hit], query: &str, _sources: &[&str], has_key: bool, answer: Option<&str>) {
    if let Some(ans) = answer {
        println!();
        println!("  \x1b[1;36m💡 {ans}\x1b[0m");
        println!();
    }
    if hits.is_empty() && answer.is_none() {
        println!("no results for \"{query}\"");
        if !has_key {
            println!();
            println!("  \x1b[2mcloud search: not configured\x1b[0m");
            println!("  \x1b[2mto enable: oobo settings set key <API_KEY>\x1b[0m");
        }
        return;
    }
    if hits.is_empty() {
        return;
    }
    for h in hits {
        let is_memory = h.source == "memory";
        let tool = h.tool.clone().unwrap_or_else(|| "-".to_string());
        let when = h
            .timestamp.map_or_else(|| "-".to_string(), relative_time);

        if is_memory {
            let author = h.author.as_deref().unwrap_or("");
            let author_suffix = if author.is_empty() {
                String::new()
            } else {
                format!(" · {author}")
            };
            println!(
                "\x1b[35m◆ memory\x1b[0m · \x1b[1m{}\x1b[0m · {when}{author_suffix}",
                h.project_name
            );
            println!("  \x1b[35m\"{}\"\x1b[0m", h.snippet);
            if let Some(sha) = &h.anchor_sha {
                println!("  anchor {sha}");
            }
        } else {
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
        }
        println!();
    }
}

fn emit_json(hits: &[Hit], query: &str, sources: &[&str], answer: Option<&str>) {
    let arr: Vec<serde_json::Value> = hits
        .iter()
        .map(|h| {
            let mut obj = serde_json::json!({
                "project": { "id": h.project_id, "name": h.project_name },
                "anchor_sha": h.anchor_sha,
                "session_id": h.session_id,
                "tool": h.tool,
                "tokens": h.tokens,
                "timestamp": h.timestamp,
                "intent": h.intent,
                "snippet": h.snippet,
                "score": h.score,
                "source": h.source,
            });
            if let Some(mid) = &h.memory_id {
                obj["memory_id"] = serde_json::json!(mid);
            }
            if let Some(author) = &h.author {
                obj["author"] = serde_json::json!(author);
            }
            obj
        })
        .collect();
    let mut json = serde_json::json!({
        "query": query,
        "sources": sources,
        "cloud_connected": sources.contains(&"remote"),
        "total_hits": hits.len(),
        "hits": arr,
        "actions": [
            { "command": "oobo anchor show <sha>", "description": "show anchor details" },
        ],
    });
    if let Some(ans) = answer {
        json["answer"] = serde_json::json!(ans);
    }
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
        assert_eq!(resolve_source("", None), Source::Local);
        assert_eq!(resolve_source("sk_test", None), Source::Both);
        assert_eq!(
            resolve_source("sk_test", Some(Source::Remote)),
            Source::Remote
        );
    }

    #[tokio::test]
    async fn remote_search_posts_contract_and_maps_hits() {
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

        let result = search_remote("sk_test", &cfg.server.url, "auth middleware", &opts).await.unwrap();

        assert!(result.answer.is_none());
        let hits = &result.hits;
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

    #[tokio::test]
    async fn remote_search_parses_answer_field() {
        let body = r#"{
          "answer": "The greeting function was built by Teddy as a staging verification test.",
          "hits": [
            {
              "source": "memory",
              "snippet": "staging verification commit",
              "anchor_sha": "c9543e8",
              "score": 0.87,
              "memory_id": "mem_abc123",
              "author": "Teddy",
              "session_ids": ["sess-1"],
              "project": { "name": "oobo-agent" },
              "timestamp": 1746450000
            }
          ]
        }"#;
        let (url, _requests) = serve_once("200 OK", body);
        let opts = Options {
            source: Some(Source::Remote),
            since: None,
            scope: Scope::Global,
            tool: None,
            limit: 10,
        };

        let result = search_remote("sk_test", &url, "why did we build the greeting function", &opts)
            .await
            .unwrap();

        assert_eq!(
            result.answer.as_deref(),
            Some("The greeting function was built by Teddy as a staging verification test.")
        );
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].source, "memory");
        assert_eq!(result.hits[0].author.as_deref(), Some("Teddy"));
        assert_eq!(result.hits[0].memory_id.as_deref(), Some("mem_abc123"));
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
