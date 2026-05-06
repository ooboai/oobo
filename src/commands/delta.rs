//! `oobo delta` — textual diff between two anchors via the remote API.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::CmdResult;
use crate::remote::payload::{DeltaRequest, DeltaResponse};

#[tracing::instrument(skip_all)]
pub async fn run(
    cfg: &Config,
    anchor_sha: Option<&str>,
    previous_sha: Option<&str>,
    full: bool,
    mode: OutputMode,
) -> CmdResult {
    let Some(project_root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };

    let resolved = crate::commands::sync::resolve(cfg, Some(&project_root));
    if !resolved.has_api_key() {
        eprintln!("error: oobo delta requires an API key. run: oobo settings set key <KEY>");
        return Ok(2);
    }

    let sha = match anchor_sha {
        Some(s) => resolve_full_sha(cfg, s).unwrap_or_else(|_| s.to_string()),
        None => resolve_head_sha(cfg)?,
    };
    let prev = previous_sha
        .map(|s| resolve_full_sha(cfg, s).unwrap_or_else(|_| s.to_string()));

    let request = DeltaRequest {
        anchor_sha: sha,
        previous_sha: prev,
        git_remote: None,
        repo_id: None,
        full,
    };

    let response = crate::remote::post_delta(
        &request,
        &resolved.api_key,
        &resolved.api_url,
        std::time::Duration::from_secs(10),
    )
    .await
    .map_err(|e| format!("delta failed: {e}"))?;

    emit(&request, &response, mode);
    Ok(0)
}

fn resolve_head_sha(cfg: &Config) -> Result<String, crate::error::CliError> {
    crate::git::proxy::run_git_capture(cfg, &["rev-parse", "HEAD"])
}

fn resolve_full_sha(cfg: &Config, short: &str) -> Result<String, crate::error::CliError> {
    crate::git::proxy::run_git_capture(cfg, &["rev-parse", short])
}

// ── emitters ────────────────────────────────────────────────────────────────

fn emit(request: &DeltaRequest, resp: &DeltaResponse, mode: OutputMode) {
    match mode {
        OutputMode::Json => emit_json(resp),
        OutputMode::Agent => emit_agent(request, resp),
        OutputMode::Tui => emit_pretty(request, resp),
    }
}

fn emit_json(resp: &DeltaResponse) {
    let value = serde_json::to_value(resp).unwrap_or(serde_json::json!({}));
    crate::utils::print_json(&value);
}

fn emit_agent(request: &DeltaRequest, resp: &DeltaResponse) {
    if let Some(cur) = &resp.current {
        let sha = cur.sha.as_deref().unwrap_or("?");
        let headline = cur.headline.as_deref().unwrap_or(cur.message.as_deref().unwrap_or("-"));
        let cat = cur.category.as_deref().unwrap_or("-");
        let cx = cur.complexity.as_deref().unwrap_or("-");
        println!("current  {sha}  [{cat}/{cx}]  {headline}");
    }
    if let Some(prev) = &resp.previous {
        let sha = prev.sha.as_deref().unwrap_or("?");
        let headline = prev.headline.as_deref().unwrap_or(prev.message.as_deref().unwrap_or("-"));
        let cat = prev.category.as_deref().unwrap_or("-");
        let cx = prev.complexity.as_deref().unwrap_or("-");
        println!("previous {sha}  [{cat}/{cx}]  {headline}");
    }
    if let Some(ch) = &resp.changes {
        println!();
        if let Some(cs) = &ch.category_shift {
            println!("category:   {} → {}", cs.from, cs.to);
        }
        if let Some(cs) = &ch.complexity_shift {
            println!("complexity: {} → {}", cs.from, cs.to);
        }
        if !ch.new_areas.is_empty() {
            println!("new areas:  {}", ch.new_areas.join(", "));
        }
        if !ch.new_techniques.is_empty() {
            println!("techniques: {}", ch.new_techniques.join(", "));
        }
        if !ch.files_new.is_empty() {
            println!("new files:  {}", ch.files_new.join(", "));
        }
        if !ch.files_continued.is_empty() {
            println!("continued:  {}", ch.files_continued.join(", "));
        }
        if let Some(n) = &ch.narrative {
            println!();
            println!("{n}");
        }
    }
    if resp.current.is_none() && resp.previous.is_none() {
        println!("no delta data for {}", request.anchor_sha);
    }
}

fn emit_pretty(request: &DeltaRequest, resp: &DeltaResponse) {
    if resp.current.is_none() && resp.previous.is_none() {
        println!("no delta data for {}", request.anchor_sha);
        return;
    }

    if let Some(cur) = &resp.current {
        print_anchor_block("Current", cur);
    }
    if let Some(prev) = &resp.previous {
        print_anchor_block("Previous", prev);
    }

    if let Some(ch) = &resp.changes {
        println!();
        println!("\x1b[1mChanges\x1b[0m");
        if let Some(cs) = &ch.category_shift {
            println!("  category:   \x1b[33m{}\x1b[0m → \x1b[36m{}\x1b[0m", cs.from, cs.to);
        }
        if let Some(cs) = &ch.complexity_shift {
            println!("  complexity: \x1b[33m{}\x1b[0m → \x1b[36m{}\x1b[0m", cs.from, cs.to);
        }
        if !ch.new_areas.is_empty() {
            println!("  new areas:  {}", ch.new_areas.join(", "));
        }
        if !ch.new_techniques.is_empty() {
            println!("  techniques: {}", ch.new_techniques.join(", "));
        }
        if !ch.files_new.is_empty() {
            println!("  \x1b[32m+ {}\x1b[0m", ch.files_new.join(", "));
        }
        if !ch.files_continued.is_empty() {
            println!("  \x1b[2m~ {}\x1b[0m", ch.files_continued.join(", "));
        }
        if let Some(n) = &ch.narrative {
            println!();
            println!("  {n}");
        }
    }

    if let Some(detail) = &resp.current_detail {
        println!();
        println!("\x1b[1mCurrent (detail)\x1b[0m");
        print_detail_block(detail);
    }
    if let Some(detail) = &resp.previous_detail {
        println!();
        println!("\x1b[1mPrevious (detail)\x1b[0m");
        print_detail_block(detail);
    }
}

fn print_anchor_block(label: &str, a: &crate::remote::payload::DeltaAnchorSummary) {
    let sha = a.sha.as_deref().unwrap_or("?");
    let msg = a.message.as_deref().unwrap_or("-");
    let author = a.author.as_deref().unwrap_or("-");
    let ts = a.timestamp.as_deref().unwrap_or("-");
    let headline = a.headline.as_deref().unwrap_or("");
    let cat = a.category.as_deref().unwrap_or("-");
    let cx = a.complexity.as_deref().unwrap_or("-");

    println!();
    println!("\x1b[1m{label}\x1b[0m  \x1b[33m{sha}\x1b[0m  {msg}");
    println!("  author: {author}  {ts}");
    if !headline.is_empty() {
        println!("  {headline}");
    }
    println!("  [{cat} · {cx}]");
}

fn print_detail_block(detail: &serde_json::Value) {
    if let Some(obj) = detail.as_object() {
        for (k, v) in obj {
            match v {
                serde_json::Value::String(s) => println!("  {k}: {s}"),
                serde_json::Value::Array(arr) => {
                    let items: Vec<String> = arr
                        .iter()
                        .map(|i| match i {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect();
                    if !items.is_empty() {
                        println!("  {k}: {}", items.join(", "));
                    }
                }
                _ => println!("  {k}: {v}"),
            }
        }
    }
}
