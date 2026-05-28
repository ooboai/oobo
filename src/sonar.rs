//! Integration with `sonar-core`. Exposes all sonar functionality
//! through `oobo search` with semantic code search capabilities.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sonar_core::index::{Mode, SonarIndex};
use sonar_core::types::{ContentType, IndexStats, SearchResult};

fn parse_mode(s: &str) -> Mode {
    match s {
        "semantic" => Mode::Semantic,
        "bm25" => Mode::Bm25,
        _ => Mode::Hybrid,
    }
}

fn parse_content_types(s: &str) -> Vec<ContentType> {
    match s {
        "docs" => vec![ContentType::Docs],
        "config" => vec![ContentType::Config],
        "all" => vec![ContentType::Code, ContentType::Docs, ContentType::Config],
        _ => vec![ContentType::Code],
    }
}

pub fn search_codebase(
    query: &str,
    path: &str,
    top_k: usize,
    mode: &str,
    content: &str,
    git_ref: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let content_types = parse_content_types(content);
    let requested_mode = parse_mode(mode);

    let mut index = if sonar_core::utils::is_git_url(path) {
        SonarIndex::from_git(path, git_ref, &[])?
    } else {
        SonarIndex::from_path_cached_with_content(Path::new(path), &content_types)?
    };

    index.set_mode(requested_mode);
    Ok(index.search(query, top_k))
}

pub fn index_codebase(path: &str, content: &str) -> Result<IndexStats, String> {
    let content_types = parse_content_types(content);
    let root = Path::new(path);
    let index = sonar_core::persist::build_and_save_content(root, &content_types)?;
    Ok(index.stats())
}

pub fn download_model(model: &str) -> Result<usize, String> {
    let embedder =
        sonar_core::embed::Embedder::from_pretrained(model).map_err(|e| e.to_string())?;
    Ok(embedder.dim())
}

pub fn watch(path: &str) -> Result<(), crate::error::CliError> {
    let root = PathBuf::from(path);
    eprintln!("Watching {}...", root.display());

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .ok();

    let content_types = vec![ContentType::Code];
    let _ = sonar_core::persist::build_and_save_content(&root, &content_types);

    let mut watcher = sonar_core::watch::FileWatcher::new(root.clone(), 500)
        .map_err(|e| crate::error::CliError::User(e.to_string()))?;

    while running.load(Ordering::SeqCst) {
        let changes = watcher.poll_changes();
        if !changes.is_empty() {
            eprintln!(
                "Re-indexing due to changes: {} files modified",
                changes.len()
            );
            match sonar_core::persist::build_and_save_content(&root, &content_types) {
                Ok(idx) => {
                    let stats = idx.stats();
                    eprintln!(
                        "Re-indexed: {} files, {} chunks.",
                        stats.indexed_files, stats.total_chunks
                    );
                }
                Err(e) => eprintln!("Re-index failed: {e}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Ok(())
}

pub fn print_savings() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let records = sonar_core::stats::read_usage().unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    let today = sonar_core::stats::calculate_savings(&records, Some(now - 86400.0));
    let week = sonar_core::stats::calculate_savings(&records, Some(now - 604800.0));
    let all = sonar_core::stats::calculate_savings(&records, None);

    println!("Token savings (estimated):");
    println!(
        "  Today:        {} tokens saved ({} calls)",
        format_number(today.saved_tokens),
        today.calls
    );
    println!(
        "  Last 7 days:  {} tokens saved ({} calls)",
        format_number(week.saved_tokens),
        week.calls
    );
    println!(
        "  All time:     {} tokens saved ({} calls)",
        format_number(all.saved_tokens),
        all.calls
    );
}

pub fn init_agent(agent: &str, force: bool) -> Result<String, String> {
    let agent_path = match agent {
        "claude" => ".claude/agents/sonar-search.md",
        "copilot" => ".github/agents/sonar-search.md",
        "cursor" => ".cursor/agents/sonar-search.md",
        "gemini" => ".gemini/agents/sonar-search.md",
        "kiro" => ".kiro/agents/sonar-search.md",
        "opencode" => ".opencode/agents/sonar-search.md",
        other => {
            return Err(format!(
                "Unknown agent '{other}'. Supported: claude, cursor, copilot, gemini, kiro, opencode"
            ));
        }
    };

    let path = PathBuf::from(agent_path);
    if path.exists() && !force {
        return Err(format!(
            "{} already exists. Use --force to overwrite.",
            path.display()
        ));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    fs::write(&path, AGENT_TEMPLATE).map_err(|e| format!("Failed to write agent file: {e}"))?;

    Ok(path.display().to_string())
}

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

const AGENT_TEMPLATE: &str = r#"---
name: sonar-search
description: Code search agent for exploring any codebase. Use for finding code by intent, locating implementations, understanding how something works, or discovering related code. Prefer over Grep/Glob/Read for any semantic or exploratory question.
tools: Bash, Read
---

Use `oobo search` to find code by describing what it does or naming a symbol/identifier, instead of grep:

```bash
oobo search "authentication flow" -p .
oobo search "getUserById" -p .
oobo search "save model to disk" -p . --top-k 10
```

The index is built on first run (and cached for subsequent runs) and invalidated automatically when files change.

Use `--mode` to control search strategy:

```bash
oobo search "parse config" -p . --mode hybrid     # default: BM25 + semantic
oobo search "parse config" -p . --mode bm25       # keyword only (fastest)
oobo search "parse config" -p . --mode semantic   # vector only
```

Use `--content` to search beyond code files:

```bash
oobo search "deployment guide" -p . --content docs      # markdown, rst, etc.
oobo search "database host port" -p . --content config  # yaml, toml, env, etc.
oobo search "authentication" -p . --content all         # code + docs + config
```

### Workflow

1. Start with `oobo search` to find relevant chunks. The index is built and cached automatically.
2. Inspect full files only when the returned chunk does not give enough context.
3. Use grep only when you need exhaustive literal matches or quick confirmation of an exact string.
"#;
