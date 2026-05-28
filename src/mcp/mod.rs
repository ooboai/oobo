//! `oobo mcp` -- stdio MCP server providing code search and engineering memory.
//!
//! Exposes tools over JSON-RPC (MCP protocol 2024-11-05):
//! - search / find_related: local code search via sonar-core
//! - recall: cloud memory search (proxies /anchors/search)
//! - ask: conversational question with synthesized answer (proxies /memory/ask)

pub mod protocol;
pub mod server;
pub mod tools;

use std::io::{self, BufRead, Write};

use server::Server;

/// Run the MCP server on stdin/stdout until EOF.
pub fn run(api_key: Option<String>, api_url: &str) -> Result<(), String> {
    use std::io::IsTerminal;

    if std::io::stdin().is_terminal() {
        eprintln!("oobo-mcp: MCP server (stdio JSON-RPC). Waiting for input...");
        eprintln!("oobo-mcp: This is meant to be launched by AI tools, not run directly.");
        eprintln!("oobo-mcp: To configure your AI tools, run: oobo mcp install");
        eprintln!("oobo-mcp: Press Ctrl+C to exit.");
        eprintln!();
    }

    let project_root = detect_project_root();
    let branch = detect_branch(&project_root);

    let has_key = api_key.as_ref().is_some_and(|k| !k.is_empty());
    if !has_key {
        eprintln!("oobo-mcp: cloud memory not configured (recall, get_context, ask unavailable)");
        eprintln!("oobo-mcp: set OOBO_API_KEY or run: oobo settings set key <KEY>");
    }

    if let Some(root) = &project_root {
        eprintln!("oobo-mcp: serving {} (branch: {})",
            std::path::Path::new(root).file_name().and_then(|n| n.to_str()).unwrap_or("unknown"),
            branch.as_deref().unwrap_or("detached"),
        );
    } else {
        eprintln!("oobo-mcp: no git repository detected (search, find_related unavailable)");
    }

    let server = Server::new(api_key, api_url.to_string(), project_root, branch);

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = protocol::JsonRpcResponse::error(
                    serde_json::Value::Null,
                    -32700,
                    format!("Parse error: {e}"),
                );
                let out = serde_json::to_string(&err_resp).unwrap_or_default();
                let _ = writeln!(stdout, "{out}");
                let _ = stdout.flush();
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let params = request.get("params").cloned();

        if let Some(response) = server.handle(&method, id, params.as_ref()) {
            let out = serde_json::to_string(&response).unwrap_or_default();
            let _ = writeln!(stdout, "{out}");
            let _ = stdout.flush();
        }
    }

    Ok(())
}

fn detect_project_root() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn detect_branch(root: &Option<String>) -> Option<String> {
    let _ = root;
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        let b = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if b == "HEAD" {
            None
        } else {
            Some(b)
        }
    } else {
        None
    }
}
