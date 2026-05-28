//! Tool schemas and handlers for the oobo MCP server.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use sonar_core::index::SonarIndex;
use sonar_core::types::ContentType;

pub fn tool_schemas(has_api_key: bool, has_repo: bool) -> Vec<Value> {
    let mut tools = Vec::new();

    if has_repo {
        tools.push(search_schema());
        tools.push(find_related_schema());
    }

    if has_api_key {
        tools.push(recall_schema());
        tools.push(get_context_schema());
        tools.push(ask_schema());
    }

    tools
}

// ── Schemas ──────────────────────────────────────────────────────────────

fn search_schema() -> Value {
    json!({
        "name": "search",
        "description": "Search code in the current repository using natural language or code queries. Uses hybrid BM25 + semantic search. Indexes are built on first use and cached.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Natural language or code query." },
                "repo": { "type": "string", "description": "Local path or https:// git URL. Defaults to current repo root." },
                "top_k": { "type": "integer", "description": "Number of results.", "default": 5 },
                "content": { "type": "string", "enum": ["code", "docs", "config", "all"], "description": "Content types to search.", "default": "code" }
            },
            "required": ["query"]
        }
    })
}

fn find_related_schema() -> Value {
    json!({
        "name": "find_related",
        "description": "Find code semantically similar to a specific file location. Use after search to explore related implementations, callers, or patterns.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to the file (relative to repo root)." },
                "line": { "type": "integer", "description": "Line number (1-indexed)." },
                "repo": { "type": "string", "description": "Local path or git URL. Defaults to current repo root." },
                "top_k": { "type": "integer", "description": "Number of similar chunks.", "default": 5 }
            },
            "required": ["file_path", "line"]
        }
    })
}

fn recall_schema() -> Value {
    json!({
        "name": "recall",
        "description": "Search engineering memory - past sessions, decisions, failures, and learnings across all projects. Returns relevant history with optional AI-synthesized answer.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What to search for (e.g. 'authentication flow', 'why did we choose Redis', 'last time deploy failed')." },
                "project": { "type": "string", "description": "Project name to scope search (omit for all projects)." },
                "since": { "type": "string", "description": "Time window (e.g. '7d', '24h', '30d'). Omit for all time." },
                "tool": { "type": "string", "description": "Filter by AI tool (claude, cursor, copilot, etc.)." },
                "limit": { "type": "integer", "description": "Max results to return.", "default": 10 }
            },
            "required": ["query"]
        }
    })
}

fn ask_schema() -> Value {
    json!({
        "name": "ask",
        "description": "Ask a question about your team's engineering work. Returns an AI-synthesized answer with sources. Use for high-level questions like 'what's the status of the auth migration' or 'why did we choose Stripe'.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "Your question about the team's engineering work." },
                "project": { "type": "string", "description": "Project name to scope the answer (omit for org-wide)." }
            },
            "required": ["question"]
        }
    })
}

fn get_context_schema() -> Value {
    json!({
        "name": "get_context",
        "description": "Get relevant engineering context for the current session. Returns recent activity, past decisions, known pitfalls, and cross-project references for the files and topics you're working on. Call this at the start of a task or when you need background on unfamiliar code.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files you're about to work on (paths relative to repo root). Context is scoped to these."
                },
                "topic": { "type": "string", "description": "Brief description of what you're working on (helps relevance ranking)." },
                "project": { "type": "string", "description": "Project name (if not inferred from repo)." },
                "budget_tokens": { "type": "integer", "default": 4000, "description": "Max tokens for the returned context. Higher = more detail, more cost." }
            }
        }
    })
}

// ── Handlers ─────────────────────────────────────────────────────────────

pub struct ToolContext {
    pub default_repo: Option<String>,
    pub api_key: Option<String>,
    pub api_url: String,
    pub index_cache: Mutex<HashMap<String, SonarIndex>>,
}

impl ToolContext {
    pub fn new(default_repo: Option<String>, api_key: Option<String>, api_url: String) -> Self {
        Self {
            default_repo,
            api_key,
            api_url,
            index_cache: Mutex::new(HashMap::new()),
        }
    }
}

pub fn dispatch(ctx: &ToolContext, tool_name: &str, params: &Value) -> Value {
    match tool_name {
        "search" => handle_search(ctx, params),
        "find_related" => handle_find_related(ctx, params),
        "recall" => handle_recall(ctx, params),
        "get_context" => handle_get_context(ctx, params),
        "ask" => handle_ask(ctx, params),
        _ => tool_error(&format!("Unknown tool: {tool_name}")),
    }
}

// ── search ───────────────────────────────────────────────────────────────

fn handle_search(ctx: &ToolContext, params: &Value) -> Value {
    let query = match params.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q.to_string(),
        _ => return tool_error("Missing required parameter: query"),
    };

    let top_k = params
        .get("top_k")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(5) as usize;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("code");

    let repo = params
        .get("repo")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .or_else(|| ctx.default_repo.clone())
        .unwrap_or_else(|| ".".to_string());

    if sonar_core::utils::is_git_url(&repo)
        && !repo.starts_with("https://")
        && !repo.starts_with("http://")
    {
        return tool_error("Only https:// and http:// git URLs are accepted.");
    }

    let content_types = parse_content_types(content);

    if let Err(e) = ensure_indexed(ctx, &repo, &content_types) {
        return tool_error(&format!("Failed to index {repo}: {e}"));
    }

    let cache = match ctx.index_cache.lock() {
        Ok(c) => c,
        Err(_) => return tool_error("Internal error: index lock poisoned"),
    };

    let index = match cache.get(&repo) {
        Some(idx) => idx,
        None => return tool_error("Internal error: index not found after build"),
    };

    let results = index.search(&query, top_k);
    if results.is_empty() {
        return tool_result(&format!("No results found for: \"{query}\""));
    }

    let formatted = sonar_core::utils::format_results(&query, &results);
    let json_str = serde_json::to_string_pretty(&formatted).unwrap_or_default();
    tool_result(&json_str)
}

// ── find_related ─────────────────────────────────────────────────────────

fn handle_find_related(ctx: &ToolContext, params: &Value) -> Value {
    let file_path = match params.get("file_path").and_then(|v| v.as_str()) {
        Some(fp) if !fp.is_empty() => fp.to_string(),
        _ => return tool_error("Missing required parameter: file_path"),
    };

    let line = match params.get("line").and_then(serde_json::Value::as_u64) {
        Some(l) => l as usize,
        None => return tool_error("Missing required parameter: line"),
    };

    let top_k = params
        .get("top_k")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(5) as usize;

    let repo = params
        .get("repo")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .or_else(|| ctx.default_repo.clone())
        .unwrap_or_else(|| ".".to_string());

    let content_types = vec![ContentType::Code];
    if let Err(e) = ensure_indexed(ctx, &repo, &content_types) {
        return tool_error(&format!("Failed to index {repo}: {e}"));
    }

    let cache = match ctx.index_cache.lock() {
        Ok(c) => c,
        Err(_) => return tool_error("Internal error: index lock poisoned"),
    };

    let index = match cache.get(&repo) {
        Some(idx) => idx,
        None => return tool_error("Internal error: index not found after build"),
    };

    let chunk = match sonar_core::utils::resolve_chunk(index.chunks(), &file_path, line) {
        Some(c) => c,
        None => {
            return tool_error(&format!(
                "No indexed chunk found at {file_path}:{line}. Ensure the file is in the index."
            ));
        }
    };

    let results = index.find_related(chunk, top_k);
    if results.is_empty() {
        return tool_result("No related code found.");
    }

    let formatted =
        sonar_core::utils::format_results(&format!("Code related to {file_path}:{line}"), &results);
    let json_str = serde_json::to_string_pretty(&formatted).unwrap_or_default();
    tool_result(&json_str)
}

// ── recall (cloud) ───────────────────────────────────────────────────────

fn handle_recall(ctx: &ToolContext, params: &Value) -> Value {
    let api_key = match &ctx.api_key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return tool_error(
                "Cloud memory not configured. Set OOBO_API_KEY environment variable or run: oobo settings set key <KEY>",
            );
        }
    };

    let query = match params.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q.to_string(),
        _ => return tool_error("Missing required parameter: query"),
    };

    let project_scope = params.get("project").and_then(|v| v.as_str());
    let since = params.get("since").and_then(|v| v.as_str());
    let tool = params.get("tool").and_then(|v| v.as_str());
    let limit = params
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(10) as usize;

    let request = crate::remote::payload::SearchRequest {
        query,
        since: since.map(std::string::ToString::to_string),
        project: Some(build_project_scope(
            project_scope,
            ctx.default_repo.as_deref(),
        )),
        tool: tool.map(std::string::ToString::to_string),
        limit,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();

    let rt = match rt {
        Ok(r) => r,
        Err(e) => return tool_error(&format!("Failed to create runtime: {e}")),
    };

    let result = rt.block_on(crate::remote::search_anchors_with_timeout(
        &request,
        &api_key,
        &ctx.api_url,
        std::time::Duration::from_secs(15),
    ));

    match result {
        Ok(response) => {
            let mut output = json!({
                "hits": response.hits.iter().map(|h| {
                    json!({
                        "project": h.project.name,
                        "anchor_sha": h.anchor_sha,
                        "tool": h.tool,
                        "timestamp": h.timestamp,
                        "intent": h.intent,
                        "snippet": h.snippet,
                        "score": h.score,
                        "source": h.source,
                    })
                }).collect::<Vec<_>>(),
                "total": response.hits.len(),
            });
            if let Some(answer) = &response.answer {
                output["answer"] = json!(answer);
            }
            tool_result(&serde_json::to_string_pretty(&output).unwrap_or_default())
        }
        Err(e) => tool_error(&format!("Cloud search failed: {e}")),
    }
}

// ── get_context (cloud) ──────────────────────────────────────────────────

fn handle_get_context(ctx: &ToolContext, params: &Value) -> Value {
    let api_key = match &ctx.api_key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return tool_error(
                "Cloud memory not configured. Set OOBO_API_KEY environment variable or run: oobo settings set key <KEY>",
            );
        }
    };

    let files = params.get("files").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
            .collect::<Vec<_>>()
    });

    let topic = params
        .get("topic")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string);
    let project_name = params
        .get("project")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string);
    let budget_tokens = params
        .get("budget_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(4000) as usize;

    let project_id = if project_name.is_none() {
        ctx.default_repo
            .as_ref()
            .map(|r| crate::project::id_for_root(r))
    } else {
        None
    };

    let request = crate::remote::payload::ContextRequest {
        files,
        topic,
        project_name,
        project_id,
        budget_tokens,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();

    let rt = match rt {
        Ok(r) => r,
        Err(e) => return tool_error(&format!("Failed to create runtime: {e}")),
    };

    let result = rt.block_on(crate::remote::get_context_with_timeout(
        &request,
        &api_key,
        &ctx.api_url,
        std::time::Duration::from_secs(15),
    ));

    match result {
        Ok(response) => {
            if response.context.is_empty() {
                return tool_result("No relevant context found for this session.");
            }

            let output = json!({
                "context": response.context.iter().map(|c| {
                    json!({
                        "type": c.item_type,
                        "relevance": c.relevance,
                        "summary": c.summary,
                        "anchor_sha": c.anchor_sha,
                        "timestamp": c.timestamp,
                    })
                }).collect::<Vec<_>>(),
                "total_tokens_used": response.total_tokens_used,
            });
            tool_result(&serde_json::to_string_pretty(&output).unwrap_or_default())
        }
        Err(e) => tool_error(&format!("Failed to get context: {e}")),
    }
}

// ── ask (cloud) ──────────────────────────────────────────────────────────

fn handle_ask(ctx: &ToolContext, params: &Value) -> Value {
    let api_key = match &ctx.api_key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return tool_error(
                "Cloud memory not configured. Set OOBO_API_KEY environment variable or run: oobo settings set key <KEY>",
            );
        }
    };

    let question = match params.get("question").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q.to_string(),
        _ => return tool_error("Missing required parameter: question"),
    };

    let project_scope = params.get("project").and_then(|v| v.as_str());

    let request = crate::remote::payload::SearchRequest {
        query: question,
        since: None,
        project: Some(build_project_scope(
            project_scope,
            ctx.default_repo.as_deref(),
        )),
        tool: None,
        limit: 5,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();

    let rt = match rt {
        Ok(r) => r,
        Err(e) => return tool_error(&format!("Failed to create runtime: {e}")),
    };

    let result = rt.block_on(crate::remote::search_anchors_with_timeout(
        &request,
        &api_key,
        &ctx.api_url,
        std::time::Duration::from_secs(30),
    ));

    match result {
        Ok(response) => {
            if let Some(answer) = &response.answer {
                let output = json!({
                    "answer": answer,
                    "sources": response.hits.iter().take(3).map(|h| {
                        json!({
                            "project": h.project.name,
                            "snippet": h.snippet,
                            "anchor_sha": h.anchor_sha,
                        })
                    }).collect::<Vec<_>>(),
                });
                tool_result(&serde_json::to_string_pretty(&output).unwrap_or_default())
            } else if !response.hits.is_empty() {
                let output = json!({
                    "answer": null,
                    "note": "No synthesized answer available. Here are the most relevant results:",
                    "hits": response.hits.iter().take(5).map(|h| {
                        json!({
                            "project": h.project.name,
                            "intent": h.intent,
                            "snippet": h.snippet,
                        })
                    }).collect::<Vec<_>>(),
                });
                tool_result(&serde_json::to_string_pretty(&output).unwrap_or_default())
            } else {
                tool_result("No relevant information found for this question.")
            }
        }
        Err(e) => tool_error(&format!("Cloud search failed: {e}")),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn build_project_scope(
    explicit_project: Option<&str>,
    default_repo: Option<&str>,
) -> crate::remote::payload::SearchProjectScope {
    if let Some(name) = explicit_project {
        crate::remote::payload::SearchProjectScope {
            kind: "project_name".to_string(),
            value: Some(name.to_string()),
        }
    } else if let Some(root) = default_repo {
        crate::remote::payload::SearchProjectScope {
            kind: "project_id".to_string(),
            value: Some(crate::project::id_for_root(root)),
        }
    } else {
        crate::remote::payload::SearchProjectScope {
            kind: "global".to_string(),
            value: None,
        }
    }
}

fn ensure_indexed(
    ctx: &ToolContext,
    repo: &str,
    content_types: &[ContentType],
) -> Result<(), String> {
    let mut cache = ctx
        .index_cache
        .lock()
        .map_err(|e| format!("Lock poisoned: {e}"))?;

    if cache.contains_key(repo) {
        return Ok(());
    }

    eprintln!("oobo-mcp: indexing {repo}...");

    let index = if sonar_core::utils::is_git_url(repo) {
        SonarIndex::from_git(repo, None, content_types)?
    } else {
        SonarIndex::from_path_cached_with_content(Path::new(repo), content_types)?
    };

    let stats = index.stats();
    eprintln!(
        "oobo-mcp: indexed {} files, {} chunks",
        stats.indexed_files, stats.total_chunks
    );

    cache.insert(repo.to_string(), index);
    Ok(())
}

fn parse_content_types(s: &str) -> Vec<ContentType> {
    match s {
        "docs" => vec![ContentType::Docs],
        "config" => vec![ContentType::Config],
        "all" => vec![ContentType::Code, ContentType::Docs, ContentType::Config],
        _ => vec![ContentType::Code],
    }
}

fn tool_result(text: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": text}]
    })
}

fn tool_error(text: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": true
    })
}
